//! Whitespace cleanup (ADR 0113): on save, of the lines edited since the file was read or
//! saved, when the setting is on; and `delete-trailing-whitespace`, of a whole buffer or its
//! regions.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. Past
//! [`TRIM_ON_THE_UI_THREAD`] of text to look at, the scan is a [`TrimJob`] the host runs on a
//! thread of its own, and what waited for it goes on in [`Editor::trim_reported`].

use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::Editor;
use super::save::Elsewhere;
use crate::buffer::{BufferId, EditError, Replacement, TrimRules, trailing_whitespace};
use crate::rope::Rope;

/// The most text a cleanup looks at on the UI thread, as isearch's budget for a key
/// (ADR 0082). Past it, the scan runs on a thread.
pub(crate) const TRIM_ON_THE_UI_THREAD: usize = 256 * 1024;

/// A scan for whitespace to clean, over an O(1) snapshot, for the host to run on a thread of
/// its own. It has no `Debug` of what it holds, which can say something of the text.
pub struct TrimJob {
    buffer: BufferId,
    revision: u64,
    text: Rope,
    ranges: Vec<Range<usize>>,
    keep: Vec<usize>,
    rules: TrimRules,
}

impl fmt::Debug for TrimJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrimJob")
            .field("buffer", &self.buffer)
            .finish_non_exhaustive()
    }
}

impl TrimJob {
    /// The buffer it scans.
    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// Finds what to delete. It takes time in the text it looks at, so not on the UI thread.
    #[must_use]
    pub fn run(self) -> TrimReport {
        let deletions = trailing_whitespace(&self.text, &self.ranges, &self.keep, self.rules);
        TrimReport {
            buffer: self.buffer,
            revision: self.revision,
            deletions,
        }
    }
}

/// What a [`TrimJob`] found, for [`Editor::trim_reported`]. `Debug` gives no offsets.
pub struct TrimReport {
    buffer: BufferId,
    revision: u64,
    deletions: Vec<Range<usize>>,
}

impl fmt::Debug for TrimReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrimReport")
            .field("buffer", &self.buffer)
            .finish_non_exhaustive()
    }
}

/// What waits for a scan.
#[derive(Clone, Debug)]
pub(super) enum Then {
    /// A save, which the person agreed may go over a file changed on disk if `over`.
    Save { over: bool },
    /// A write to a new place (ADR 0087).
    Write { path: PathBuf, elsewhere: Elsewhere },
    /// `delete-trailing-whitespace`, which saves nothing.
    Command { ranges: Vec<Range<usize>> },
}

/// A scan under way, and what waits for it.
#[derive(Debug)]
struct Waiting {
    buffer: BufferId,
    then: Then,
    /// The agent channel ran what waits, so the save is the agent's.
    for_agent: bool,
}

/// The setting, the scans to run and what waits for them.
#[derive(Debug, Default)]
pub(super) struct Trims {
    on_save: bool,
    jobs: Vec<TrimJob>,
    waiting: Vec<Waiting>,
}

/// Whether a cleanup is done, or waits for a scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Trimmed {
    /// Done, and whether it changed the text.
    Done(bool),
    Waiting,
}

/// Whether `path` names a Markdown file, by its name until major modes come.
fn is_markdown(path: Option<&Path>) -> bool {
    path.and_then(Path::extension)
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

impl Editor {
    /// Turns whitespace cleanup on save on or off. It applies from the next save.
    pub fn set_trim_whitespace(&mut self, on: bool) {
        self.trims.on_save = on;
    }

    /// Deletes the trailing spaces and tabs of the selected window's regions, if they show,
    /// or of its whole buffer, and the blank lines at its end if what it covers reaches
    /// them. Returns whether the text changed. Past [`TRIM_ON_THE_UI_THREAD`] it scans on a
    /// thread, and changes the text when [`Editor::trim_reported`] takes the answer.
    ///
    /// # Errors
    ///
    /// [`EditError::ReadOnly`], and nothing changes.
    pub fn delete_trailing_whitespace(&mut self) -> Result<bool, EditError> {
        let (window, buffer) = self.current();
        if let Some(reason) = buffer.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        let selections = &window.selections;
        let mut ranges: Vec<Range<usize>> = if selections.mark_active().is_active() {
            selections
                .iter()
                .map(|selection| selection.range())
                .collect()
        } else {
            std::iter::once(0..buffer.text().len()).collect()
        };
        ranges.sort_unstable_by_key(|range| range.start);
        let id = window.buffer();
        match self.trim(id, ranges.clone(), Vec::new(), Then::Command { ranges }) {
            Trimmed::Done(changed) => Ok(changed),
            Trimmed::Waiting => Ok(false),
        }
    }

    /// Cleans the lines of `id` edited since its file was read or saved, if the setting is
    /// on, for `then`, a save, and forgets where it was edited either way. The cursors of the
    /// windows showing it keep what is before them on their lines.
    pub(super) fn trim_for_save(&mut self, id: BufferId, then: Then) -> Trimmed {
        let Some(buffer) = self.buffer_mut(id) else {
            return Trimmed::Done(false);
        };
        let ranges = buffer.edited().ranges().to_vec();
        if !self.trims.on_save || ranges.is_empty() {
            if let Some(buffer) = self.buffer_mut(id) {
                buffer.clear_edited();
            }
            return Trimmed::Done(false);
        }
        let keep = self.cursors_of(id);
        let trimmed = self.trim(id, ranges, keep, then);
        if trimmed == Trimmed::Waiting
            && let Some(buffer) = self.buffer_mut(id)
        {
            // A second save is refused while this one waits, as while one runs (ADR 0059).
            buffer.set_saving(true);
        }
        trimmed
    }

    /// Cleans `ranges` of `id`, at once within the budget or else through a scan on a thread,
    /// which `then` waits for.
    fn trim(
        &mut self,
        id: BufferId,
        ranges: Vec<Range<usize>>,
        keep: Vec<usize>,
        then: Then,
    ) -> Trimmed {
        let Some(buffer) = self.buffer(id) else {
            return Trimmed::Done(false);
        };
        let rules = TrimRules {
            markdown: is_markdown(buffer.path()),
            final_lines: true,
        };
        let job = TrimJob {
            buffer: id,
            revision: buffer.revision(),
            text: buffer.text().clone(),
            ranges,
            keep,
            rules,
        };
        let looked_at: usize = job.ranges.iter().map(ExactSizeIterator::len).sum();
        if looked_at > TRIM_ON_THE_UI_THREAD {
            self.trims.jobs.push(job);
            self.trims.waiting.push(Waiting {
                buffer: id,
                then,
                for_agent: self.runs_for_agent(),
            });
            self.message("Cleaning up whitespace…");
            return Trimmed::Waiting;
        }
        let deletions = job.run().deletions;
        let changed = self.delete_ranges(id, &deletions);
        if !matches!(then, Then::Command { .. })
            && let Some(buffer) = self.buffer_mut(id)
        {
            buffer.clear_edited();
        }
        Trimmed::Done(changed)
    }

    /// The cursors of every window showing `id`, ascending.
    fn cursors_of(&self, id: BufferId) -> Vec<usize> {
        let mut cursors: Vec<usize> = self
            .windows
            .iter()
            .filter(|window| window.buffer() == id)
            .flat_map(|window| window.selections.iter().map(|selection| selection.cursor))
            .collect();
        cursors.sort_unstable();
        cursors
    }

    /// Deletes `deletions` from `id`, as one edit. Returns whether the text changed.
    fn delete_ranges(&mut self, id: BufferId, deletions: &[Range<usize>]) -> bool {
        if deletions.is_empty() {
            return false;
        }
        let edits: Vec<Replacement<'_>> = deletions
            .iter()
            .map(|range| Replacement {
                range: range.clone(),
                text: "",
            })
            .collect();
        self.apply_edits(id, &edits).is_ok()
    }

    /// The scans to run, once each, for the host.
    pub fn take_trim_jobs(&mut self) -> Vec<TrimJob> {
        std::mem::take(&mut self.trims.jobs)
    }

    /// Whether a scan is under way for `id`.
    #[cfg(test)]
    pub(crate) fn trim_waits(&self, id: BufferId) -> bool {
        self.trims
            .waiting
            .iter()
            .any(|waiting| waiting.buffer == id)
    }

    /// Takes in what a scan found, and goes on with what waited for it, in an undo step of
    /// its own: the cleanup, then for a save its final newline and its job. If the text
    /// changed since the scan began, it scans again, and the host runs the new job. Returns,
    /// once what waited has gone on, whether a save it queued is the agent's, for the host to
    /// start it as such.
    pub fn trim_reported(&mut self, report: TrimReport) -> Option<bool> {
        let index = self
            .trims
            .waiting
            .iter()
            .position(|waiting| waiting.buffer == report.buffer)?;
        let Some(buffer) = self.buffer(report.buffer) else {
            self.trims.waiting.swap_remove(index);
            return None;
        };
        if buffer.revision() != report.revision {
            self.trim_again(index);
            return None;
        }
        let Waiting {
            buffer: id,
            then,
            for_agent,
        } = self.trims.waiting.swap_remove(index);
        self.begin_undo_group();
        self.delete_ranges(id, &report.deletions);
        match then {
            Then::Command { .. } => {}
            Then::Save { over } => {
                if let Some(buffer) = self.buffer_mut(id) {
                    buffer.clear_edited();
                    buffer.set_saving(false);
                }
                if let Err(refused) = self.save_after_trim(id, over) {
                    self.message(format!("Not saved: {refused}"));
                }
            }
            Then::Write { path, elsewhere } => {
                if let Some(buffer) = self.buffer_mut(id) {
                    buffer.clear_edited();
                    buffer.set_saving(false);
                }
                if let Err(refused) = self.write_after_trim(id, path, elsewhere) {
                    self.message(format!("Not written: {refused}"));
                }
            }
        }
        self.end_undo_group();
        Some(for_agent)
    }

    /// The scan for `id` could not run: what waited for it is dropped, and a save of it does
    /// not happen, which the echo area says.
    pub fn trim_failed(&mut self, id: BufferId) {
        let Some(index) = self
            .trims
            .waiting
            .iter()
            .position(|waiting| waiting.buffer == id)
        else {
            return;
        };
        let waiting = self.trims.waiting.swap_remove(index);
        if matches!(waiting.then, Then::Command { .. }) {
            self.message("Whitespace not cleaned: the scan could not start");
            return;
        }
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_saving(false);
        }
        self.message("Not saved: the whitespace scan could not start");
    }

    /// Scans the waiting entry at `index` again, over the text as it is now: the ranges edited
    /// since, for a save, or the same ones followed through the edits, for the command.
    fn trim_again(&mut self, index: usize) {
        let id = self.trims.waiting[index].buffer;
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let ranges = match &self.trims.waiting[index].then {
            Then::Command { ranges } => {
                let len = buffer.text().len();
                ranges
                    .iter()
                    .map(|range| range.start.min(len)..range.end.min(len))
                    .collect()
            }
            Then::Save { .. } | Then::Write { .. } => buffer.edited().ranges().to_vec(),
        };
        let keep = self.cursors_of(id);
        let rules = TrimRules {
            markdown: is_markdown(buffer.path()),
            final_lines: true,
        };
        self.trims.jobs.push(TrimJob {
            buffer: id,
            revision: buffer.revision(),
            text: buffer.text().clone(),
            ranges,
            keep,
            rules,
        });
    }
}
