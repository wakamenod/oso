//! Wrapping on a worker (ADR 0100): a window's [`WrapMap`] gives each command a budget,
//! keeps what it leaves stale, and hands out one [`WrapJob`] at a time for that, or for a new
//! width. A report comes back for the snapshot it was made from, and the edits made since are
//! applied to it before it replaces the window's rows.
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
/// charged [`SPLICE_COST`] more, about 4 ms whatever the script (ADR 0100).
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

/// The most commands kept for a report, past which it is dropped and its job runs again.
const SINCE_COMMANDS: usize = 64;

/// The most bytes the edits kept for a report may have rebuilt.
const SINCE_BYTES: usize = 1024 * 1024;

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
    /// after the last. Once the budget is spent, the rest of the command is joined in one go.
    pub fn edited_command(
        &self,
        splices: &[Splice],
        budget: &mut usize,
    ) -> (Self, Vec<Range<usize>>) {
        let mut wrap = self.clone();
        let mut stale: Vec<Range<usize>> = Vec::new();
        for (index, splice) in splices.iter().enumerate() {
            let rest = &splices[index..];
            if *budget == 0 && rest.len() > 1 {
                let (first, last) = (&rest[0], &rest[rest.len() - 1]);
                let old = changed_before(rest);
                let delta = rest
                    .iter()
                    .map(|splice| splice.edit.new.len() as isize - splice.edit.old.len() as isize)
                    .sum();
                for splice in rest {
                    for range in &mut stale {
                        *range = shifted(range, &splice.edit);
                    }
                }
                let (spent, range) = wrap.spent((&first.before, &last.after), old, delta);
                stale.push(range);
                return (spent, stale);
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
    /// Rewraps a stale range.
    Range(Range<usize>),
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
            Work::Range(range) => self.wrap.rewrapped_unless(&self.display, range, &stop)?,
            Work::Columns(columns) => {
                WrapSnapshot::with_width_unless(&self.display, wrap_width(columns), &stop)?
            }
        };
        (!stop()).then_some(WrapReport { generation, wrap })
    }
}

/// The rows a [`WrapJob`] made, for the window it came from.
pub struct WrapReport {
    generation: u64,
    wrap: WrapSnapshot,
}

impl fmt::Debug for WrapReport {
    /// Leaves out the rows.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WrapReport")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// A window's wrap of its buffer: the rows, what is stale in them, and the job under way.
pub(crate) struct WrapMap {
    wrap: WrapSnapshot,
    /// The window's width, which the rows may not have caught up with.
    columns: usize,
    /// What a job should do next: rewrap a stale range, or wrap at the window's width.
    wanted: Option<Work>,
    /// The generation of the job whose report is awaited, and that the job reads to stop.
    live: Arc<AtomicU64>,
    running: bool,
    /// The commands since the running job's snapshot, `None` once past the bounds.
    since: Option<Vec<Vec<Splice>>>,
    since_bytes: usize,
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
            since: None,
            since_bytes: 0,
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
        let wrap = self.wrap.clone();
        self.wrap = self.apply(&wrap, splices);
        if self.running
            && let Some(since) = &mut self.since
        {
            self.since_bytes += splices
                .iter()
                .map(|splice| splice.edit.new.len())
                .sum::<usize>();
            since.push(splices.to_vec());
            if since.len() > SINCE_COMMANDS || self.since_bytes > SINCE_BYTES {
                self.since = None;
            }
        }
    }

    /// Applies `splices` to `wrap`, noting what they leave stale.
    fn apply(&mut self, wrap: &WrapSnapshot, splices: &[Splice]) -> WrapSnapshot {
        if let Some(Work::Range(range)) = &mut self.wanted {
            for splice in splices {
                *range = shifted(range, &splice.edit);
            }
        }
        let mut budget = COMMAND_BUDGET;
        let (wrap, stale) = wrap.edited_command(splices, &mut budget);
        for range in stale {
            self.want_range(range);
        }
        wrap
    }

    /// Adds `range` to what a job should rewrap.
    fn want_range(&mut self, range: Range<usize>) {
        self.wanted = match self.wanted.take() {
            None => Some(Work::Range(range)),
            Some(Work::Range(wanted)) => Some(Work::Range(
                wanted.start.min(range.start)..wanted.end.max(range.end),
            )),
            Some(columns @ Work::Columns(_)) => Some(columns),
        };
    }

    /// Makes the window `columns` wide. Its rows keep their old width until a job reports.
    pub(crate) fn set_columns(&mut self, columns: usize) {
        if columns != self.columns {
            self.columns = columns;
            self.wanted = Some(Work::Columns(columns));
        }
    }

    /// The next job, for the buffer as `display` shows it now: none while one runs, unless a
    /// new width supersedes it.
    pub(crate) fn take_job(&mut self, display: &DisplaySnapshot) -> Option<WrapJob> {
        let supersedes = matches!(self.wanted, Some(Work::Columns(_)));
        if self.wanted.is_none() || (self.running && !supersedes) {
            return None;
        }
        let work = self.wanted.take()?;
        let generation = self.live.fetch_add(1, Ordering::Relaxed) + 1;
        self.running = true;
        self.since = Some(Vec::new());
        self.since_bytes = 0;
        Some(WrapJob {
            wrap: self.wrap.clone(),
            display: display.clone(),
            work,
            generation,
            live: Arc::clone(&self.live),
        })
    }

    /// Takes a job's report: the commands since its snapshot are applied to it, and it
    /// replaces the rows. A superseded report is dropped; one past the bounds of what is kept
    /// since is dropped too, and the window's width wrapped again.
    pub(crate) fn take_report(&mut self, report: WrapReport) -> bool {
        if report.generation != self.live.load(Ordering::Relaxed) || !self.running {
            return false;
        }
        self.running = false;
        let Some(since) = self.since.take() else {
            self.wanted = Some(Work::Columns(self.columns));
            return false;
        };
        let mut wrap = report.wrap;
        for splices in &since {
            wrap = self.apply(&wrap, splices);
        }
        self.wrap = wrap;
        true
    }
}

#[cfg(test)]
mod tests;
