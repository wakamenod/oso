//! A window's wrap of its buffer (ADR 0100, ADR 0102, ADR 0103): the rows, what is stale and
//! not yet wrapped near the view in them, the command's budget, and the one job under way, for
//! the ranges left stale or a new width.
//!
//! Must not depend on: threads, the clock or the window; the window holds it and the app runs
//! its jobs.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::job::{Hulls, MAX_STALE, WHOLE_PAST, Work, shifted};
use super::{COMMAND_BUDGET, Splice, WrapJob, WrapReport, WrapSnapshot, wrap_width};
use crate::display_map::DisplaySnapshot;

/// A window's wrap of its buffer: the rows, what is stale in them, and the job under way. A
/// clone shares the generation, so that a window can follow an edit on a clone and swap it in
/// (ADR 0053).
#[derive(Clone)]
pub(crate) struct WrapMap {
    pub(super) wrap: WrapSnapshot,
    /// The window's width, which the rows may not have caught up with.
    columns: usize,
    /// What a job should do next: rewrap stale ranges, or wrap at the window's width.
    wanted: Option<Work>,
    /// The generation of the job whose report is awaited, and that the job reads to stop.
    live: Arc<AtomicU64>,
    running: bool,
    /// What the commands since the running job's snapshot changed.
    since: Hulls,
    /// What the running command may still read on the UI thread, its edits and the rows
    /// near the view together.
    pub(super) budget: usize,
    /// The provisional rows not yet wrapped near the view, sorted and apart: every row of
    /// a new width, and what edits left stale (`view.rs`).
    pub(super) unrealized: Vec<Range<usize>>,
}

impl WrapMap {
    /// The rows of `display` at `columns`, wrapped at once.
    #[cfg(test)]
    pub(crate) fn new(display: &DisplaySnapshot, columns: usize) -> Self {
        Self::with_rows(WrapSnapshot::new(display, columns), columns)
    }

    /// The rows of `display` at `columns` before any is wrapped: one a logical line, made in
    /// O(log n), with a job wanted for all of them. The rows near the view are then wrapped
    /// with [`WrapMap::realize`].
    pub(crate) fn provisional(display: &DisplaySnapshot, columns: usize) -> Self {
        let mut map = Self::with_rows(
            WrapSnapshot::provisional(display, wrap_width(columns)),
            columns,
        );
        if map.wrap.wraps() {
            map.wanted = Some(Work::Columns(columns));
            map.unrealized = std::iter::once(0..display.len()).collect();
        }
        map
    }

    fn with_rows(wrap: WrapSnapshot, columns: usize) -> Self {
        Self {
            wrap,
            columns,
            wanted: None,
            live: Arc::new(AtomicU64::new(0)),
            running: false,
            since: Hulls::default(),
            budget: COMMAND_BUDGET,
            unrealized: Vec::new(),
        }
    }

    /// Starts over from `display`, which no splice led to, as a reload's text: provisional
    /// rows, with a job for all of them that supersedes the running one.
    pub(crate) fn reset(&mut self, display: &DisplaySnapshot) {
        let columns = self.columns;
        let fresh = Self::provisional(display, columns);
        self.wrap = fresh.wrap;
        self.unrealized = fresh.unrealized;
        self.since = Hulls::default();
        // A job of the old text must not report; the next job supersedes it.
        self.running = false;
        self.wanted = fresh.wanted;
    }

    /// Gives the next command its budget.
    pub(crate) fn refill(&mut self) {
        self.budget = COMMAND_BUDGET;
    }

    /// The rows as they are now, some perhaps provisional.
    pub(crate) fn snapshot(&self) -> &WrapSnapshot {
        &self.wrap
    }

    /// Whether some rows are provisional, or of another width, until a job reports.
    #[cfg(test)]
    pub(crate) fn is_stale(&self) -> bool {
        self.wanted.is_some() || self.running
    }

    /// Follows one command's splices, in the order they were applied, within what is left of
    /// the command's budget; what it leaves stale waits for a job.
    pub(crate) fn edit(&mut self, splices: &[Splice]) {
        if let Some(Work::Ranges(ranges)) = &mut self.wanted {
            follow_splices(ranges, splices);
        }
        follow_splices(&mut self.unrealized, splices);
        if self.running {
            for splice in splices {
                self.since.grow(&splice.edit);
            }
        }
        let (wrap, stale) = self.wrap.edited_command(splices, &mut self.budget);
        self.wrap = wrap;
        for range in stale {
            self.want_range(range);
        }
    }

    /// Adds `range` to the provisional rows not yet wrapped near the view.
    pub(super) fn add_unrealized(&mut self, range: Range<usize>) {
        add_apart(&mut self.unrealized, range);
    }

    /// Adds `range` to the stale ranges, kept sorted and apart, and to the rows not yet
    /// wrapped near the view.
    pub(super) fn want_range(&mut self, range: Range<usize>) {
        self.add_unrealized(range.clone());
        if matches!(self.wanted, Some(Work::Columns(_))) {
            return;
        }
        let Work::Ranges(ranges) = self.wanted.get_or_insert_with(|| Work::Ranges(Vec::new()))
        else {
            return;
        };
        add_apart(ranges, range);
    }

    /// Makes the window `columns` wide, `display` showing its buffer. Its rows keep their old
    /// breaks until a job reports, but those near the view, which [`WrapMap::realize`] wraps
    /// at the new width. Below [`MIN_COLUMNS`](super::MIN_COLUMNS), nothing wraps, and the
    /// rows are exact at once.
    pub(crate) fn set_columns(&mut self, columns: usize, display: &DisplaySnapshot) {
        if columns == self.columns {
            return;
        }
        let (was, width) = (self.wrap.wraps(), wrap_width(columns));
        self.columns = columns;
        if width == usize::MAX || !was {
            self.reset(display);
            return;
        }
        self.wrap.width = width;
        // The running job's report would bring the old width back.
        self.running = false;
        self.since = Hulls::default();
        self.wanted = Some(Work::Columns(columns));
        self.unrealized = std::iter::once(0..display.len()).collect();
    }

    /// The next job, for the buffer as `display` shows it now: none while one runs, unless a
    /// new width supersedes it. Stale ranges of more than [`WHOLE_PAST`] bytes in all make a
    /// wrap of the whole buffer.
    pub(crate) fn take_job(&mut self, display: &DisplaySnapshot) -> Option<WrapJob> {
        let supersedes = matches!(self.wanted, Some(Work::Columns(_)));
        if self.wanted.is_none() || (self.running && !supersedes) {
            return None;
        }
        let work = match self.wanted.take()? {
            Work::Ranges(ranges)
                if ranges.iter().map(|range| range.len()).sum::<usize>() > WHOLE_PAST =>
            {
                Work::Columns(self.columns)
            }
            work => work,
        };
        let generation = self.live.fetch_add(1, Ordering::Relaxed) + 1;
        self.running = true;
        self.since = Hulls::default();
        Some(WrapJob {
            wrap: self.wrap.clone(),
            display: display.clone(),
            work,
            generation,
            live: Arc::downgrade(&self.live),
        })
    }

    /// Takes a job's report, with `display` the buffer as it shows now. What the commands since
    /// its snapshot changed is joined to its rows as one provisional span, left stale for the
    /// next job, and they replace the rows. A superseded report is dropped.
    ///
    /// A report from another map's job is not taken: [`WrapMap::owns`] says whose it is.
    pub(crate) fn take_report(&mut self, report: WrapReport, display: &DisplaySnapshot) -> bool {
        if !self.owns(&report) || report.generation != self.live.load(Ordering::Relaxed) {
            return false;
        }
        if !self.running {
            return false;
        }
        self.running = false;
        let since = std::mem::take(&mut self.since);
        let (wrap, stale) = since.join(report.wrap, (&report.display, display));
        // What is still provisional: what a job is wanted for.
        self.unrealized = match &self.wanted {
            None => Vec::new(),
            Some(Work::Columns(_)) => std::iter::once(0..display.len()).collect(),
            Some(Work::Ranges(ranges)) => ranges.clone(),
        };
        for range in stale {
            self.want_range(range);
        }
        self.wrap = wrap;
        true
    }

    /// Whether `report` came from a job of this map, or of a clone of it.
    pub(crate) fn owns(&self, report: &WrapReport) -> bool {
        std::ptr::eq(report.owner.as_ptr(), Arc::as_ptr(&self.live))
    }
}

/// `ranges`, sorted and apart, moved along with `splices`, in the order they were applied;
/// those that grew to meet become one.
fn follow_splices(ranges: &mut Vec<Range<usize>>, splices: &[Splice]) {
    if ranges.is_empty() {
        return;
    }
    for splice in splices {
        for range in ranges.iter_mut() {
            *range = shifted(range, &splice.edit);
        }
    }
    let moved = std::mem::take(ranges);
    for range in moved {
        match ranges.last_mut() {
            Some(last) if last.end >= range.start => last.end = last.end.max(range.end),
            _ => ranges.push(range),
        }
    }
}

/// Adds `range` to `ranges`, kept sorted and apart, at most [`MAX_STALE`]: past it the two
/// nearest join.
fn add_apart(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    let at = ranges.partition_point(|other| other.end < range.start);
    let mut merged = range;
    while let Some(next) = ranges.get(at).filter(|next| next.start <= merged.end) {
        merged = merged.start.min(next.start)..merged.end.max(next.end);
        ranges.remove(at);
    }
    ranges.insert(at, merged);
    if ranges.len() > MAX_STALE {
        let nearest = (1..ranges.len())
            .min_by_key(|&index| ranges[index].start - ranges[index - 1].end)
            .unwrap_or(1);
        let next = ranges.remove(nearest);
        ranges[nearest - 1].end = next.end;
    }
}

#[cfg(test)]
mod tests;
