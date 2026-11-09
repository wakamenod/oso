//! Wrapping on a worker (ADR 0100, ADR 0102): a window's [`WrapMap`] gives each command a
//! budget, keeps the ranges it leaves stale, and hands out one [`WrapJob`] at a time for them,
//! or for a new width. A report comes back for the snapshot it was made from; what the edits
//! made since changed is joined to it as one provisional span, left stale for the next job.
//!
//! Must not depend on: threads, the clock or the window; the app runs the job.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{WrapSnapshot, wrap_width};
use crate::display_map::DisplaySnapshot;
use crate::rope::RopeEdit;

/// What a command's rewraps may read on the UI thread, in bytes of rows, each splice
/// charged [`SPLICE_COST`] more: about 4 ms in ASCII and 5.5 in CJK (ADR 0102).
#[cfg(not(test))]
pub const COMMAND_BUDGET: usize = 128 * 1024;
/// Small in tests, whose texts are small too, so that their edits run out of it.
#[cfg(test)]
pub const COMMAND_BUDGET: usize = 24;

/// What a splice's rewrap costs before it reads a row, as bytes of rows: finding where it
/// starts, where it may stop and splicing the tree, each a seek into a long line.
#[cfg(not(test))]
pub(crate) const SPLICE_COST: usize = 2 * 1024;
/// Small in tests, as [`COMMAND_BUDGET`] is.
#[cfg(test)]
pub(crate) const SPLICE_COST: usize = 4;

/// How many splices of a command past its budget join the old rows one by one, keeping the
/// old rows between them, about 15 µs each on a long line; the rest join in one go.
#[cfg(not(test))]
const SPENT_JOINS: usize = 64;
/// Small in tests, so that they join in one go too.
#[cfg(test)]
const SPENT_JOINS: usize = 2;

/// The most stale ranges kept apart; past it the two nearest join.
const MAX_STALE: usize = 64;

/// Past this many stale bytes, a job wraps the whole buffer, which keeps no rows aside, rather
/// than holding every row of the ranges.
#[cfg(not(test))]
const WHOLE_PAST: usize = 4 * 1024 * 1024;
/// Small in tests, so that they wrap the whole buffer too.
#[cfg(test)]
const WHOLE_PAST: usize = 64;

/// One edit of a command, with the display snapshots from before and after it.
#[derive(Clone)]
pub struct Splice {
    /// The buffer as it showed before the edit.
    pub before: DisplaySnapshot,
    /// The buffer as it shows after it.
    pub after: DisplaySnapshot,
    /// What the rope rebuilt.
    pub edit: RopeEdit,
}

impl fmt::Debug for Splice {
    /// Leaves out the edit's offsets.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Splice").finish_non_exhaustive()
    }
}

impl WrapSnapshot {
    /// The rows after one command's `splices`, in the order they were applied, reading at most
    /// `budget` bytes of rows, which it takes from, and the ranges left stale, in the offsets
    /// after the last. Past the budget, each splice's rows are provisional from the row that
    /// held it to the next old boundary, and the old rows between splices stay; past
    /// [`SPENT_JOINS`] such splices, the rest of the command joins in one go.
    pub fn edited_command(
        &self,
        splices: &[Splice],
        budget: &mut usize,
    ) -> (Self, Vec<Range<usize>>) {
        let mut wrap = self.clone();
        let mut stale: Vec<Range<usize>> = Vec::new();
        let mut spent = 0;
        for (index, splice) in splices.iter().enumerate() {
            let rest = &splices[index..];
            if *budget == 0 {
                spent += 1;
            }
            if spent > SPENT_JOINS && rest.len() > 1 {
                let (first, last) = (&rest[0], &rest[rest.len() - 1]);
                let delta = rest
                    .iter()
                    .map(|splice| splice.edit.new.len() as isize - splice.edit.old.len() as isize)
                    .sum();
                for splice in rest {
                    for range in &mut stale {
                        *range = shifted(range, &splice.edit);
                    }
                }
                let (joined, range) = wrap.spent(
                    (&first.before, &last.after),
                    changed_before(rest),
                    (delta, 0),
                );
                stale.extend(range);
                return (joined, stale);
            }
            for range in &mut stale {
                *range = shifted(range, &splice.edit);
            }
            let (edited, range) =
                wrap.edited_within(&splice.before, &splice.after, &splice.edit, budget);
            wrap = edited;
            stale.extend(range);
        }
        (wrap, stale)
    }
}

/// What `splices` changed, as one range in the offsets from before the first: each splice's
/// range taken back through those applied before it.
fn changed_before(splices: &[Splice]) -> Range<usize> {
    let back = |mut at: usize, applied: &[Splice]| {
        for splice in applied.iter().rev() {
            let edit = &splice.edit;
            let delta = edit.new.len() as isize - edit.old.len() as isize;
            if at >= edit.new.end {
                at = at.checked_add_signed(-delta).unwrap_or(0);
            } else if at > edit.new.start {
                at = edit.old.end;
            }
        }
        at
    };
    let (mut start, mut end) = (usize::MAX, 0);
    for (index, splice) in splices.iter().enumerate() {
        let applied = &splices[..index];
        start = start.min(back(splice.edit.old.start, applied));
        end = end.max(back(splice.edit.old.end, applied));
    }
    start.min(end)..end
}

/// `range` moved along with `edit`, and grown to hold it where they meet.
fn shifted(range: &Range<usize>, edit: &RopeEdit) -> Range<usize> {
    let delta = edit.new.len() as isize - edit.old.len() as isize;
    let shift = |at: usize| at.checked_add_signed(delta).unwrap_or(0);
    if edit.old.end <= range.start {
        shift(range.start)..shift(range.end)
    } else if edit.old.start < range.end {
        range.start.min(edit.new.start)..shift(range.end).max(edit.new.end)
    } else {
        range.clone()
    }
}

/// What a job does.
#[derive(Clone)]
enum Work {
    /// Rewraps stale ranges, apart and in order.
    Ranges(Vec<Range<usize>>),
    /// Wraps everything for a window this many columns wide.
    Columns(usize),
}

/// Work for the app to run off the UI thread. It stops early, giving no report, once a newer
/// job of the same window supersedes it.
pub struct WrapJob {
    wrap: WrapSnapshot,
    display: DisplaySnapshot,
    work: Work,
    generation: u64,
    live: Arc<AtomicU64>,
}

impl fmt::Debug for WrapJob {
    /// Leaves out the ranges and rows (see the module docs of [`super`]).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WrapJob")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl WrapJob {
    /// Runs the job, or gives up once it is superseded, also when it finishes.
    pub fn run(self) -> Option<WrapReport> {
        let (generation, live) = (self.generation, Arc::clone(&self.live));
        let stop = move || live.load(Ordering::Relaxed) != generation;
        let wrap = match self.work {
            Work::Ranges(ranges) => {
                let mut wrap = self.wrap;
                // A rewrap that ran past later ranges, as on a line with tabs, did them too.
                let mut reached = 0;
                for range in ranges {
                    if range.end <= reached {
                        continue;
                    }
                    let start = range.start.max(reached);
                    let (rewrapped, to) =
                        wrap.rewrapped_unless(&self.display, start..range.end, &stop)?;
                    (wrap, reached) = (rewrapped, to);
                }
                wrap
            }
            Work::Columns(columns) => {
                WrapSnapshot::with_width_unless(&self.display, wrap_width(columns), &stop)?
            }
        };
        (!stop()).then_some(WrapReport {
            generation,
            wrap,
            display: self.display,
        })
    }
}

/// The rows a [`WrapJob`] made, for the window it came from, and the buffer as it showed them.
pub struct WrapReport {
    generation: u64,
    wrap: WrapSnapshot,
    display: DisplaySnapshot,
}

impl fmt::Debug for WrapReport {
    /// Leaves out the rows.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WrapReport")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// What the edits since a job's snapshot changed in one place: a range in the offsets now,
/// and how much longer they made it.
#[derive(Clone, Copy)]
struct Hull {
    start: usize,
    end: usize,
    delta: isize,
}

/// The places the edits since a job's snapshot changed, sorted and apart, at most
/// [`MAX_STALE`], so that two far apart do not make all between them provisional.
#[derive(Default)]
struct Hulls(Vec<Hull>);

impl Hulls {
    /// Takes `edit`, made after those already kept.
    fn grow(&mut self, edit: &RopeEdit) {
        let delta = edit.new.len() as isize - edit.old.len() as isize;
        let mut grown = Hull {
            start: edit.new.start,
            end: edit.new.end,
            delta,
        };
        let shift = |at: usize| at.checked_add_signed(delta).unwrap_or(0);
        let mut kept = Vec::with_capacity(self.0.len() + 1);
        for hull in self.0.drain(..) {
            if hull.end < edit.old.start {
                kept.push(hull);
            } else if hull.start > edit.old.end {
                kept.push(Hull {
                    start: shift(hull.start),
                    end: shift(hull.end),
                    delta: hull.delta,
                });
            } else {
                // They meet: one place.
                grown.start = grown.start.min(hull.start);
                grown.end = grown.end.max(shift(hull.end));
                grown.delta += hull.delta;
            }
        }
        let at = kept.partition_point(|hull| hull.start < grown.start);
        kept.insert(at, grown);
        if kept.len() > MAX_STALE {
            let nearest = (1..kept.len())
                .min_by_key(|&index| kept[index].start - kept[index - 1].end)
                .unwrap_or(1);
            let next = kept.remove(nearest);
            let hull = &mut kept[nearest - 1];
            hull.end = next.end;
            hull.delta += next.delta;
        }
        self.0 = kept;
    }

    /// Joins each place to `wrap`, rows made from `before`, with `display` the buffer now,
    /// from the last back so that what precedes each reads as `before` showed it. Says the
    /// stale ranges, in the offsets now.
    fn join(
        &self,
        wrap: WrapSnapshot,
        (before, display): (&DisplaySnapshot, &DisplaySnapshot),
    ) -> (WrapSnapshot, Vec<Range<usize>>) {
        let mut wrap = wrap;
        let mut stale = Vec::new();
        let mut base: isize = self.0.iter().map(|hull| hull.delta).sum();
        for hull in self.0.iter().rev() {
            base -= hull.delta;
            let back = |at: usize| at.checked_add_signed(-base).unwrap_or(0);
            let start = back(hull.start);
            let end = back(hull.end)
                .checked_add_signed(-hull.delta)
                .unwrap_or(0)
                .max(start);
            let (joined, range) = wrap.spent((before, display), start..end, (hull.delta, base));
            wrap = joined;
            stale.extend(range);
        }
        (wrap, stale)
    }
}

/// A window's wrap of its buffer: the rows, what is stale in them, and the job under way.
pub(crate) struct WrapMap {
    wrap: WrapSnapshot,
    /// The window's width, which the rows may not have caught up with.
    columns: usize,
    /// What a job should do next: rewrap stale ranges, or wrap at the window's width.
    wanted: Option<Work>,
    /// The generation of the job whose report is awaited, and that the job reads to stop.
    live: Arc<AtomicU64>,
    running: bool,
    /// What the commands since the running job's snapshot changed.
    since: Hulls,
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "a window holds it from ADR 0100's next step")
)]
impl WrapMap {
    /// The rows of `display` at `columns`, wrapped at once.
    pub(crate) fn new(display: &DisplaySnapshot, columns: usize) -> Self {
        Self {
            wrap: WrapSnapshot::new(display, columns),
            columns,
            wanted: None,
            live: Arc::new(AtomicU64::new(0)),
            running: false,
            since: Hulls::default(),
        }
    }

    /// The rows as they are now, some perhaps provisional.
    pub(crate) fn snapshot(&self) -> &WrapSnapshot {
        &self.wrap
    }

    /// Whether some rows are provisional, or of another width, until a job reports.
    pub(crate) fn is_stale(&self) -> bool {
        self.wanted.is_some() || self.running
    }

    /// Follows one command's splices, in the order they were applied, within the command's
    /// budget; what it leaves stale waits for a job.
    pub(crate) fn edit(&mut self, splices: &[Splice]) {
        if let Some(Work::Ranges(ranges)) = &mut self.wanted {
            for splice in splices {
                for range in ranges.iter_mut() {
                    *range = shifted(range, &splice.edit);
                }
            }
            // Ranges that grew to meet become one.
            let shifted = std::mem::take(ranges);
            for range in shifted {
                match ranges.last_mut() {
                    Some(last) if last.end >= range.start => last.end = last.end.max(range.end),
                    _ => ranges.push(range),
                }
            }
        }
        if self.running {
            for splice in splices {
                self.since.grow(&splice.edit);
            }
        }
        let mut budget = COMMAND_BUDGET;
        let (wrap, stale) = self.wrap.edited_command(splices, &mut budget);
        self.wrap = wrap;
        for range in stale {
            self.want_range(range);
        }
    }

    /// Adds `range` to the stale ranges, kept sorted and apart.
    fn want_range(&mut self, range: Range<usize>) {
        if matches!(self.wanted, Some(Work::Columns(_))) {
            return;
        }
        let Work::Ranges(ranges) = self.wanted.get_or_insert_with(|| Work::Ranges(Vec::new()))
        else {
            return;
        };
        let at = ranges.partition_point(|other| other.end < range.start);
        let mut merged = range;
        while let Some(next) = ranges.get(at).filter(|next| next.start <= merged.end) {
            merged = merged.start.min(next.start)..merged.end.max(next.end);
            ranges.remove(at);
        }
        ranges.insert(at, merged);
        if ranges.len() > MAX_STALE {
            // The two nearest join.
            let nearest = (1..ranges.len())
                .min_by_key(|&index| ranges[index].start - ranges[index - 1].end)
                .unwrap_or(1);
            let next = ranges.remove(nearest);
            ranges[nearest - 1].end = next.end;
        }
    }

    /// Makes the window `columns` wide. Its rows keep their old width until a job reports.
    pub(crate) fn set_columns(&mut self, columns: usize) {
        if columns != self.columns {
            self.columns = columns;
            self.wanted = Some(Work::Columns(columns));
        }
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
            live: Arc::clone(&self.live),
        })
    }

    /// Takes a job's report, with `display` the buffer as it shows now. What the commands since
    /// its snapshot changed is joined to its rows as one provisional span, left stale for the
    /// next job, and they replace the rows. A superseded report is dropped.
    pub(crate) fn take_report(&mut self, report: WrapReport, display: &DisplaySnapshot) -> bool {
        if report.generation != self.live.load(Ordering::Relaxed) || !self.running {
            return false;
        }
        self.running = false;
        let since = std::mem::take(&mut self.since);
        let (wrap, stale) = since.join(report.wrap, (&report.display, display));
        for range in stale {
            self.want_range(range);
        }
        self.wrap = wrap;
        true
    }
}

#[cfg(test)]
mod tests;
