//! Wrapping a command's splices within a budget, and on a worker past it (ADR 0100, ADR 0102):
//! the [`WrapJob`] a window's [`WrapMap`](super::WrapMap) hands out, its [`WrapReport`], and
//! the places the edits since a job's snapshot changed, joined to its rows when it reports.
//!
//! Must not depend on: threads, the clock or the window; the app runs the job.

use std::fmt;
use std::ops::Range;
use std::sync::Weak;
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

/// How many of a command's splices a rewrap reads the display snapshots of: each charges at
/// least [`SPLICE_COST`], so the budget is spent within `COMMAND_BUDGET / SPLICE_COST`, and the
/// first [`SPENT_JOINS`] after that join one by one. The rest join in one go, which reads only
/// the first's `before` and the last's `after`, so an edit may hand those past this many
/// placeholder snapshots, and keep no snapshot of each (ADR 0103).
pub(crate) const SPLICES_READ: usize = COMMAND_BUDGET / SPLICE_COST + SPENT_JOINS + 1;

/// The most stale ranges kept apart; past it the two nearest join.
pub(super) const MAX_STALE: usize = 64;

/// Past this many stale bytes, a job wraps the whole buffer, which keeps no rows aside, rather
/// than holding every row of the ranges.
#[cfg(not(test))]
pub(super) const WHOLE_PAST: usize = 4 * 1024 * 1024;
/// Small in tests, so that they wrap the whole buffer too.
#[cfg(test)]
pub(super) const WHOLE_PAST: usize = 64;

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
            // Its snapshots are read here, and every splice's past it only by its edit.
            debug_assert!(index < SPLICES_READ, "a snapshot read past the bound");
            if spent > SPENT_JOINS {
                let (first, last) = (&rest[0], &rest[rest.len() - 1]);
                let (before, after, delta) = changed(rest);
                // In one go, as walking every range through every splice would cost their
                // product: what precedes the change stays, what follows moves by its length,
                // and what meets it takes it in.
                for range in &mut stale {
                    if range.end < before.start {
                        continue;
                    }
                    if range.start > before.end {
                        *range = range.start.checked_add_signed(delta).unwrap_or(0)
                            ..range.end.checked_add_signed(delta).unwrap_or(0);
                    } else {
                        let end = range.end.max(before.end).checked_add_signed(delta);
                        *range = range.start.min(after.start)..end.unwrap_or(0).max(after.end);
                    }
                }
                let (joined, range) = wrap.spent((&first.before, &last.after), before, (delta, 0));
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

/// What `splices` changed, as one range in the offsets from before the first and the same in
/// the offsets after the last, and how much longer they made the text. One range grows over
/// each splice in turn, so that it holds every one: nothing before its start changed, and
/// what follows its end only moved. O(splices).
fn changed(splices: &[Splice]) -> (Range<usize>, Range<usize>, isize) {
    let mut range: Option<Range<usize>> = None;
    let mut delta = 0;
    for splice in splices {
        let edit = &splice.edit;
        delta += edit.new.len() as isize - edit.old.len() as isize;
        range = Some(match range {
            None => edit.new.clone(),
            Some(range) => {
                let moved = shifted(&range, edit);
                moved.start.min(edit.new.start)..moved.end.max(edit.new.end)
            }
        });
    }
    let after = range.unwrap_or(0..0);
    let end = after
        .end
        .checked_add_signed(-delta)
        .unwrap_or(0)
        .max(after.start);
    #[cfg(test)]
    {
        let old = changed_before(splices);
        assert!(
            after.start <= old.start && end >= old.end,
            "the one range holds every splice's"
        );
    }
    (after.start..end, after, delta)
}

/// What `splices` changed, as one range in the offsets from before the first: each splice's
/// range taken back through those applied before it. O(splices²): only tests check
/// [`changed`] against it.
#[cfg(test)]
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
pub(super) fn shifted(range: &Range<usize>, edit: &RopeEdit) -> Range<usize> {
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
pub(super) enum Work {
    /// Rewraps stale ranges, apart and in order.
    Ranges(Vec<Range<usize>>),
    /// Wraps everything for a window this many columns wide.
    Columns(usize),
}

/// Work for the app to run off the UI thread. It stops early, giving no report, once a newer
/// job of the same window supersedes it.
pub struct WrapJob {
    pub(super) wrap: WrapSnapshot,
    pub(super) display: DisplaySnapshot,
    pub(super) work: Work,
    pub(super) generation: u64,
    /// The generation of its map's newest job, gone with the map.
    pub(super) live: Weak<AtomicU64>,
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
    /// Runs the job, or gives up once it is superseded or its map is gone, also when it
    /// finishes.
    pub fn run(self) -> Option<WrapReport> {
        let (generation, live) = (self.generation, Weak::clone(&self.live));
        let stop = move || {
            live.upgrade()
                .is_none_or(|live| live.load(Ordering::Relaxed) != generation)
        };
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
            owner: self.live,
            wrap,
            display: self.display,
        })
    }
}

/// The rows a [`WrapJob`] made, for the window it came from, and the buffer as it showed them.
pub struct WrapReport {
    pub(super) generation: u64,
    /// The map whose job made it, so that no other map takes it.
    pub(super) owner: Weak<AtomicU64>,
    pub(super) wrap: WrapSnapshot,
    pub(super) display: DisplaySnapshot,
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
#[derive(Clone, Default)]
pub(super) struct Hulls(Vec<Hull>);

impl Hulls {
    /// Takes `edit`, made after those already kept.
    pub(super) fn grow(&mut self, edit: &RopeEdit) {
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
    /// from the last back so that what precedes each reads as `before` showed it. Places whose
    /// rows meet, a row before one reaching the next one's rows, join as one. Says the stale
    /// ranges, in the offsets now.
    pub(super) fn join(
        &self,
        wrap: WrapSnapshot,
        (before, display): (&DisplaySnapshot, &DisplaySnapshot),
    ) -> (WrapSnapshot, Vec<Range<usize>>) {
        // Each place in the offsets `before` has, with how much longer it made the text.
        let mut places: Vec<(usize, usize, isize)> = Vec::with_capacity(self.0.len());
        let mut lower: isize = 0;
        for hull in &self.0 {
            let back = |at: usize| at.checked_add_signed(-lower).unwrap_or(0);
            let start = back(hull.start);
            let end = back(hull.end)
                .checked_add_signed(-hull.delta)
                .unwrap_or(0)
                .max(start);
            lower += hull.delta;
            // The first row a join here reads: the row before the one that holds it.
            let row = wrap.row_of(before, start).saturating_sub(1);
            let first = wrap.row_span(before, row).0;
            match places.last_mut() {
                Some(last) if wrap.old_boundary(before, last.1) >= first => {
                    last.1 = last.1.max(end);
                    last.2 += hull.delta;
                }
                _ => places.push((start, end, hull.delta)),
            }
        }
        let mut wrap = wrap;
        let mut stale = Vec::new();
        let mut base: isize = places.iter().map(|place| place.2).sum();
        for &(start, end, delta) in places.iter().rev() {
            base -= delta;
            let (joined, range) = wrap.spent((before, display), start..end, (delta, base));
            wrap = joined;
            stale.extend(range);
        }
        (wrap, stale)
    }
}
