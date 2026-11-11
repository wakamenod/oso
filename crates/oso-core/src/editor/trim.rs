//! Whitespace cleanup (ADR 0113): on save, of the lines edited since the file was read or
//! saved, when the setting is on; and `delete-trailing-whitespace`, of a whole buffer or its
//! regions.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. Past
//! [`TRIM_ON_THE_UI_THREAD`] of work, the scan is a [`TrimJob`] the host runs on a thread of
//! its own, and what waited for it goes on in [`Editor::trim_reported`].

use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::Editor;
use super::follow::clip_selections;
use super::save::{Elsewhere, SaveRefused};
use crate::buffer::{
    BufferId, EditError, Reads, Replacement, Tracking, Trim, TrimRules, as_spans,
    trailing_whitespace,
};
use crate::rope::Rope;
use crate::selection::{OffsetEdit, SelectionSet, follow_ranges};
use crate::sum_tree::Bias;

/// The most work a cleanup does on the UI thread, in bytes of text, as isearch's budget for
/// a key (ADR 0082): the ranges' text, [`PER_RANGE`] for each range, and what it reads back
/// over. Past it, the scan runs on a thread.
pub(crate) const TRIM_ON_THE_UI_THREAD: usize = 256 * 1024;

/// What a range costs besides its text, in bytes of text: a seek, and a window read back.
const PER_RANGE: usize = 64;

/// Past this many deletions, a cleanup goes in as a few replacements of spans, built off the
/// UI thread when the scan ran on one, rather than as a splice each.
const MANY_DELETIONS: usize = 4096;

/// Names a scan, so that its report goes to what waits for it, however many wait for one
/// buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrimId(Tracking);

/// A scan for whitespace to clean, over an O(1) snapshot, for the host to run on a thread of
/// its own. It has no `Debug` of what it holds, which can say something of the text.
pub struct TrimJob {
    id: TrimId,
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
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl TrimJob {
    /// The scan it is, for [`Editor::trim_failed`] if its thread cannot start.
    pub fn id(&self) -> TrimId {
        self.id
    }

    /// The buffer it scans.
    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// Finds what to delete. It takes time in the text it looks at, so not on the UI thread.
    #[must_use]
    pub fn run(self) -> TrimReport {
        let trim = self.find(&mut Reads::unbounded()).unwrap_or_default();
        let spans =
            (trim.deletions.len() > MANY_DELETIONS).then(|| as_spans(&self.text, &trim.deletions));
        TrimReport {
            id: self.id,
            revision: self.revision,
            trim,
            spans,
        }
    }

    fn find(&self, reads: &mut Reads) -> Result<Trim, crate::buffer::TooLong> {
        trailing_whitespace(&self.text, &self.ranges, &self.keep, self.rules, reads)
    }
}

/// What a [`TrimJob`] found, for [`Editor::trim_reported`]. `Debug` gives no offsets.
pub struct TrimReport {
    id: TrimId,
    revision: u64,
    trim: Trim,
    /// The deletions as spans to replace, when there are many.
    spans: Option<Vec<(Range<usize>, String)>>,
}

impl fmt::Debug for TrimReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrimReport")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// What waits for a scan.
#[derive(Clone, Debug)]
pub(super) enum Then {
    /// A save, which the person agreed may go over a file changed on disk if `over`.
    Save { over: bool },
    /// A write to a new place (ADR 0087), whose name says whether it is Markdown.
    Write { path: PathBuf, elsewhere: Elsewhere },
    /// `delete-trailing-whitespace`, which saves nothing. Its ranges follow the edits in the
    /// buffer, tracked under the scan's id.
    Command,
}

/// A scan under way, and what waits for it.
#[derive(Debug)]
struct Waiting {
    id: TrimId,
    buffer: BufferId,
    then: Then,
    /// The agent channel ran what waits, so the save is the agent's.
    for_agent: bool,
}

/// The setting, the scans to run and what waits for them.
#[derive(Debug, Default)]
pub(super) struct Trims {
    on_save: bool,
    next_id: Tracking,
    jobs: Vec<TrimJob>,
    waiting: Vec<Waiting>,
    /// Buffers whose agent's save began to wait for a scan, not yet taken by the host.
    agent_saves: Vec<BufferId>,
}

/// How what waited for a scan went on, for the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrimOutcome {
    /// The buffer scanned.
    pub buffer: BufferId,
    /// The agent channel ran what waited, so a save it queued is the agent's.
    pub for_agent: bool,
    /// A save or a write that waited queued no job, for this reason. `None` for the command,
    /// and for a save that queued its job.
    pub refused: Option<SaveRefused>,
}

/// Whether a cleanup is done, or waits for a scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Trimmed {
    /// Done: whether it changed the text, and the whitespace cursors kept, which stays
    /// edited once a save takes the text.
    Done {
        changed: bool,
        kept: Vec<Range<usize>>,
    },
    Waiting,
}

impl Trimmed {
    pub(super) const NOTHING: Self = Self::Done {
        changed: false,
        kept: Vec::new(),
    };
}

/// Whether `path` names a Markdown file, by its name until major modes come.
fn is_markdown(path: Option<&Path>) -> bool {
    path.and_then(Path::extension)
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

/// `ranges` moved through `deletions`, both ascending and apart.
fn after_deletions(mut ranges: Vec<Range<usize>>, deletions: &[Range<usize>]) -> Vec<Range<usize>> {
    let edits: Vec<OffsetEdit> = deletions
        .iter()
        .map(|deletion| OffsetEdit {
            start: deletion.start,
            old_len: deletion.len(),
            new_len: 0,
        })
        .collect();
    follow_ranges(&mut ranges, &edits);
    ranges
}

impl Editor {
    /// A new id for ranges tracked in a buffer, shared by cleanup commands and query-replace.
    pub(super) fn next_tracking(&mut self) -> Tracking {
        self.trims.next_id += 1;
        self.trims.next_id
    }

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
        let markdown = is_markdown(buffer.path());
        match self.trim(id, ranges, Vec::new(), markdown, Then::Command) {
            Trimmed::Done { changed, .. } => Ok(changed),
            Trimmed::Waiting => Ok(false),
        }
    }

    /// Cleans the lines of `id` edited since its file was read or saved, if the setting is
    /// on, for `then`, a save. The cursors of the windows showing it keep what is before them
    /// on their lines. The save forgets where the buffer was edited once it takes the text.
    pub(super) fn trim_for_save(&mut self, id: BufferId, then: Then) -> Trimmed {
        let Some(buffer) = self.buffer(id) else {
            return Trimmed::NOTHING;
        };
        let ranges = buffer.edited_ranges().to_vec();
        if !self.trims.on_save || ranges.is_empty() {
            return Trimmed::NOTHING;
        }
        // A write to a new place follows the rule of the name it takes.
        let markdown = match &then {
            Then::Write { path, .. } => is_markdown(Some(path)),
            Then::Save { .. } | Then::Command => is_markdown(buffer.path()),
        };
        let keep = self.cursors_of(id);
        let trimmed = self.trim(id, ranges, keep, markdown, then);
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
        markdown: bool,
        then: Then,
    ) -> Trimmed {
        let scan = TrimId(self.next_tracking());
        let Some(buffer) = self.buffer(id) else {
            return Trimmed::NOTHING;
        };
        let job = TrimJob {
            id: scan,
            buffer: id,
            revision: buffer.revision(),
            text: buffer.text().clone(),
            ranges,
            keep,
            rules: TrimRules {
                markdown,
                final_lines: true,
            },
        };
        let cost = job
            .ranges
            .iter()
            .map(|range| range.len().saturating_add(PER_RANGE))
            .fold(0, usize::saturating_add);
        let found = (cost <= TRIM_ON_THE_UI_THREAD)
            .then(|| {
                job.find(&mut Reads::at_most(TRIM_ON_THE_UI_THREAD - cost))
                    .ok()
            })
            .flatten();
        let Some(trim) = found else {
            self.wait_for(job, then);
            return Trimmed::Waiting;
        };
        let changed = self.delete_ranges(id, &trim.deletions, None);
        Trimmed::Done {
            changed,
            kept: after_deletions(trim.kept, &trim.deletions),
        }
    }

    /// Leaves `job` for the host, with `then` waiting for it.
    fn wait_for(&mut self, job: TrimJob, then: Then) {
        let (id, buffer) = (job.id, job.buffer);
        let for_agent = self.runs_for_agent();
        if for_agent && !matches!(then, Then::Command) {
            self.trims.agent_saves.push(buffer);
        }
        if matches!(then, Then::Command)
            && let Some(buffer) = self.buffer_mut(buffer)
        {
            buffer.track(id.0, job.ranges.clone());
        }
        self.trims.jobs.push(job);
        self.trims.waiting.push(Waiting {
            id,
            buffer,
            then,
            for_agent,
        });
        self.message("Cleaning up whitespace…");
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

    /// Deletes `deletions` from `id`, as one edit. Many go in as `spans`, or spans built
    /// here, and every window's cursors and marks then follow the deletions themselves, not
    /// the spans, which would take them to a span's edge. Returns whether the text changed.
    fn delete_ranges(
        &mut self,
        id: BufferId,
        deletions: &[Range<usize>],
        spans: Option<Vec<(Range<usize>, String)>>,
    ) -> bool {
        if deletions.is_empty() {
            return false;
        }
        if deletions.len() <= MANY_DELETIONS {
            let edits: Vec<Replacement<'_>> = deletions
                .iter()
                .map(|range| Replacement {
                    range: range.clone(),
                    text: "",
                })
                .collect();
            return self.apply_edits(id, &edits).is_ok();
        }
        let Some(buffer) = self.buffer(id) else {
            return false;
        };
        let spans = spans.unwrap_or_else(|| as_spans(buffer.text(), deletions));
        let steps: Vec<OffsetEdit> = deletions
            .iter()
            .map(|deletion| OffsetEdit {
                start: deletion.start,
                old_len: deletion.len(),
                new_len: 0,
            })
            .collect();
        let followed: Vec<(usize, SelectionSet)> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, window)| window.buffer() == id)
            .map(|(index, window)| {
                let mut selections = window.selections.clone();
                selections.follow_edits(&steps, Bias::Left);
                (index, selections)
            })
            .collect();
        let edits: Vec<Replacement<'_>> = spans
            .iter()
            .map(|(range, text)| Replacement {
                range: range.clone(),
                text,
            })
            .collect();
        if self.apply_edits(id, &edits).is_err() {
            return false;
        }
        let Some(display) = self.buffer(id).map(crate::buffer::Buffer::display) else {
            return true;
        };
        for (index, mut selections) in followed {
            clip_selections(&mut selections, &display, &[], true, None);
            self.windows[index].selections = selections;
        }
        true
    }

    /// The buffers whose save, run for the agent channel, waits for a scan since the last
    /// call: the agent's reply waits for that save's end, as for any save of its own.
    pub fn take_agent_trim_saves(&mut self) -> Vec<BufferId> {
        std::mem::take(&mut self.trims.agent_saves)
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
    /// once what waited has gone on, how it went: whether a save it queued is the agent's,
    /// for the host to start it as such, or why a save queued nothing.
    pub fn trim_reported(&mut self, report: TrimReport) -> Option<TrimOutcome> {
        let index = self
            .trims
            .waiting
            .iter()
            .position(|waiting| waiting.id == report.id)?;
        let id = self.trims.waiting[index].buffer;
        let Some(buffer) = self.buffer(id) else {
            self.trims.waiting.swap_remove(index);
            return None;
        };
        if buffer.revision() != report.revision {
            self.trim_again(index);
            return None;
        }
        let Waiting {
            id: scan,
            buffer: id,
            then,
            for_agent,
        } = self.trims.waiting.swap_remove(index);
        let Trim { deletions, kept } = report.trim;
        self.begin_undo_group();
        self.delete_ranges(id, &deletions, report.spans);
        let kept = after_deletions(kept, &deletions);
        let went = match then {
            Then::Command => {
                if let Some(buffer) = self.buffer_mut(id) {
                    buffer.untrack(scan.0);
                }
                Ok(false)
            }
            Then::Save { over } => {
                if let Some(buffer) = self.buffer_mut(id) {
                    buffer.set_saving(false);
                }
                self.save_after_trim(id, over, kept)
            }
            Then::Write { path, elsewhere } => {
                if let Some(buffer) = self.buffer_mut(id) {
                    buffer.set_saving(false);
                }
                self.write_after_trim(id, path, elsewhere, kept)
            }
        };
        self.end_undo_group();
        let refused = went.err();
        if let Some(refused) = refused {
            self.message(format!("Not saved: {refused}"));
        }
        Some(TrimOutcome {
            buffer: id,
            for_agent,
            refused,
        })
    }

    /// The scan `scan` could not run: what waited for it is dropped, and a save of it does
    /// not happen, which the echo area says.
    pub fn trim_failed(&mut self, scan: TrimId) {
        let Some(index) = self
            .trims
            .waiting
            .iter()
            .position(|waiting| waiting.id == scan)
        else {
            return;
        };
        let waiting = self.trims.waiting.swap_remove(index);
        let buffer = self.buffer_mut(waiting.buffer);
        if matches!(waiting.then, Then::Command) {
            if let Some(buffer) = buffer {
                buffer.untrack(scan.0);
            }
            self.message("Whitespace not cleaned: the scan could not start");
            return;
        }
        if let Some(buffer) = buffer {
            buffer.set_saving(false);
        }
        self.message("Not saved: the whitespace scan could not start");
    }

    /// Scans the waiting entry at `index` again, over the text as it is now: the ranges edited
    /// since, for a save, or its own ranges as the edits moved them, for the command.
    fn trim_again(&mut self, index: usize) {
        let Waiting {
            id: scan,
            buffer: id,
            then,
            ..
        } = &self.trims.waiting[index];
        let (scan, id) = (*scan, *id);
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let (ranges, markdown) = match then {
            Then::Command => (
                buffer.tracked(scan.0).unwrap_or_default().to_vec(),
                is_markdown(buffer.path()),
            ),
            Then::Save { .. } => (buffer.edited_ranges().to_vec(), is_markdown(buffer.path())),
            Then::Write { path, .. } => (buffer.edited_ranges().to_vec(), is_markdown(Some(path))),
        };
        let keep = if matches!(then, Then::Command) {
            Vec::new()
        } else {
            self.cursors_of(id)
        };
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        self.trims.jobs.push(TrimJob {
            id: scan,
            buffer: id,
            revision: buffer.revision(),
            text: buffer.text().clone(),
            ranges,
            keep,
            rules: TrimRules {
                markdown,
                final_lines: true,
            },
        });
    }
}
