//! What a save and an exit need from the editor state (ADR 0059).
//!
//! Must not depend on: threads or the file system, which the host owns, or
//! `command` and `keymap`.
//!
//! [`Editor::start_save`] decides whether the selected window's buffer can be saved,
//! ensures its final newline and queues a [`SaveJob`] with an O(1) snapshot. The host
//! takes the job after the command, writes the file on a thread of its own, and
//! reports back with [`Editor::save_finished`] or [`Editor::save_failed`]. The queue
//! is what lets tests run a save with no thread at all.
//!
//! [`Editor::confirm_exit`] is the guard on exiting with work at stake: the exit command
//! asks a question, and the answer runs it again, confirmed (ADR 0072). A buffer whose file
//! changed on disk asks the same way before a save goes over it.

use std::fmt;
use std::path::{Path, PathBuf};

use super::Editor;
use super::follow::clip_selections;
use crate::buffer::{Baseline, Buffer, BufferId, ContentId, FileStamp, ReadOnly, Replacement};
use crate::rope::Rope;

/// One save for the host to run. Its text is an O(1) snapshot, which later edits do
/// not change. It has no `Debug`, so that its text cannot reach a log.
pub struct SaveJob {
    /// The buffer to report back about.
    pub buffer: BufferId,
    /// The path the buffer visits, which may be a symbolic link.
    pub path: PathBuf,
    /// The text to write.
    pub text: Rope,
    /// The file starts with a BOM, which is not in the text.
    pub bom: bool,
    /// The state of the text, for [`Editor::save_finished`].
    pub content: ContentId,
    /// Where the file must still be and what it must still hold to be replaced. A
    /// buffer the host never told about has none, and its save refuses.
    pub baseline: Option<Baseline>,
    /// The buffer is unmodified, so write only if the file has gone from where it was.
    /// Whether it has needs the disk, so the host finds out.
    pub only_if_gone: bool,
    /// The text goes to `path`, a new place for the buffer, and not where its file was
    /// (`C-x C-w`, ADR 0087). `baseline` is then `None`: the host works out where `path` leads.
    pub elsewhere: Option<Elsewhere>,
}

/// What a write to a new place may find there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Elsewhere {
    /// The regular file the person agreed to write over. With `None`, nothing may be there.
    pub over: Option<Over>,
    /// Where the other buffers' files are, which the new place must not be: one file is never
    /// in two buffers (ADR 0087).
    pub open: Vec<PathBuf>,
    /// The name the text takes in the path if it is a directory: the buffer's file's, or the
    /// buffer's own (ADR 0087).
    pub name: String,
}

/// A file a write found at a new place, which the person agreed to write over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Over {
    /// What it was then.
    pub found: FileStamp,
    /// Where the path led then. A path that now leads elsewhere writes nothing.
    pub location: PathBuf,
}

/// Why a save did not start. Nothing was written, and nothing changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveRefused {
    /// The buffer is read-only, for this reason (ADR 0022).
    ReadOnly(ReadOnly),
    /// A save of this buffer is running. A second one is refused and not queued.
    AlreadySaving,
    /// The buffer holds what the disk does.
    Unmodified,
    /// The buffer visits no file.
    NoFile,
    /// A save found the file changed on disk since it was read. A save over it needs the
    /// person to agree first (ADR 0072).
    ChangedOnDisk,
    /// A write to a new place found a file there. Writing over it needs the person to agree
    /// first (ADR 0087).
    Exists,
    /// The new place is another buffer's file.
    OpenElsewhere,
}

impl fmt::Display for SaveRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ReadOnly(ReadOnly::Binary) => "Cannot save a binary file",
            Self::ReadOnly(ReadOnly::InvalidUtf8) => "Cannot save a file with invalid UTF-8",
            Self::ReadOnly(ReadOnly::Loading) => "Cannot save a file that is still loading",
            Self::ReadOnly(ReadOnly::Internal) => "Cannot save an internal buffer",
            Self::AlreadySaving => "This buffer is already being saved; save again when it ends",
            Self::Unmodified => "(No changes need to be saved)",
            Self::NoFile => "Buffer has no file name",
            Self::ChangedOnDisk => "The file changed on disk since it was read",
            Self::Exists => "A file is there already",
            Self::OpenElsewhere => "That file is open in another buffer",
        })
    }
}

/// What an exit would put at risk, which is why it was refused this time. A question about
/// exiting says this, and the answer then exits only if no more is at risk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitRefused {
    /// Buffers that differ from their file, or have no file.
    pub modified: usize,
    /// Saves still running. A buffer being saved usually counts as modified too.
    pub saving: usize,
}

impl fmt::Display for ExitRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plural = |count: usize| if count == 1 { "" } else { "s" };
        let modified = self.modified;
        let saving = self.saving;
        if modified > 0 {
            write!(f, "{modified} modified buffer{}", plural(modified))?;
        }
        if modified > 0 && saving > 0 {
            f.write_str(" and ")?;
        }
        if saving > 0 {
            write!(f, "{saving} save{} running", plural(saving))?;
        }
        Ok(())
    }
}

/// Why `buffer` cannot be saved now, if it cannot. Unmodified comes before no file,
/// as in Emacs, where `C-x C-s` in an untouched `*scratch*` has nothing to do.
pub(super) fn refusal(buffer: &Buffer) -> Option<SaveRefused> {
    if let Some(reason) = buffer.read_only() {
        Some(SaveRefused::ReadOnly(reason))
    } else if buffer.is_saving() {
        Some(SaveRefused::AlreadySaving)
    } else if !buffer.is_modified() && !may_have_lost_its_file(buffer) {
        Some(SaveRefused::Unmodified)
    } else if buffer.path().is_none() {
        Some(SaveRefused::NoFile)
    } else {
        None
    }
}

/// Whether `buffer` read a file that may be gone by now. Its text would then exist nowhere
/// else, so a save of it is worth asking the disk about, as Emacs does.
fn may_have_lost_its_file(buffer: &Buffer) -> bool {
    buffer.path().is_some()
        && buffer
            .baseline()
            .is_some_and(|baseline| baseline.file.is_some())
}

/// The line break `buffer` needs at its end, if any: a text with something in it that
/// does not end in one (ADR 0030). O(log n).
pub(super) fn missing_final_newline(buffer: &Buffer) -> Option<&'static str> {
    let text = buffer.text();
    let last = text.chunks_in_range(text.len().saturating_sub(1)..text.len());
    let needed = !text.is_empty() && !last.last().is_some_and(|last| last.ends_with('\n'));
    needed.then(|| buffer.file_info().line_ending.text())
}

impl Editor {
    /// Starts saving the selected window's buffer: ensures its final newline, as an
    /// edit of its own that undoes as one step, and queues a job with a snapshot of
    /// the result for the host. Returns whether the text changed. The cursors stay
    /// before the new line break, as Emacs's do.
    ///
    /// # Errors
    ///
    /// Why the save did not start. Nothing changes.
    pub fn start_save(&mut self) -> Result<bool, SaveRefused> {
        self.queue_save(false)
    }

    /// Starts saving as [`Editor::start_save`] does, over a file a save found changed on
    /// disk, once the person has agreed. The save replaces it only if it is still the file
    /// that save found, so a change made since is kept and asked about again.
    ///
    /// # Errors
    ///
    /// Why the save did not start. Nothing changes.
    pub fn start_save_over(&mut self) -> Result<bool, SaveRefused> {
        self.queue_save(true)
    }

    pub(super) fn queue_save(&mut self, over: bool) -> Result<bool, SaveRefused> {
        let id = self.window().buffer();
        let buffer = self.current().1;
        if let Some(refused) = refusal(buffer) {
            return Err(refused);
        }
        // An unmodified buffer has nothing to save over a file with: it writes only if the file
        // has gone, and asks nothing.
        let found = buffer.save_over().filter(|_| buffer.is_modified());
        // An unmodified buffer stays as it is unless the file has gone, which only the
        // host can tell, and then it holds what the file held.
        let only_if_gone = !buffer.is_modified();
        let line_break = missing_final_newline(buffer).filter(|_| !only_if_gone);
        // `y` saves over the file of the buffer the question was asked of, and no other.
        let asked = self.save_over_asked.take();
        if found.is_some() && (!over || asked != Some(id)) {
            self.save_over_asked = Some(id);
            return Err(SaveRefused::ChangedOnDisk);
        }
        if let Some(line_break) = line_break {
            self.insert_final_newline(id, line_break);
        }
        if let Some(buffer) = self.buffer_mut(id)
            && let Some(path) = buffer.path().map(Path::to_path_buf)
        {
            let job = SaveJob {
                buffer: id,
                path,
                text: buffer.text().clone(),
                bom: buffer.file_info().bom,
                content: buffer.content_id(),
                baseline: buffer.baseline().cloned().map(|mut baseline| {
                    // The file found replaces only if it is still there as found. A file that
                    // goes meanwhile gets back the mode it had when read, not the one found.
                    if let Some(found) = found {
                        let mode = baseline.file.map_or(found.mode, |read| read.mode);
                        baseline.file = Some(FileStamp { mode, ..found });
                    }
                    baseline
                }),
                only_if_gone,
                elsewhere: None,
            };
            buffer.set_saving(true);
            self.save_jobs.push(job);
        }
        Ok(line_break.is_some())
    }

    /// Adds `line_break` at the end of `id`, the selected window's buffer, and
    /// keeps the cursors that were at the end where they were.
    pub(super) fn insert_final_newline(&mut self, id: BufferId, line_break: &str) {
        let old_end = self.current().1.text().len();
        let edit = Replacement {
            range: old_end..old_end,
            text: line_break,
        };
        // The caller knows the buffer exists and is not read-only.
        if self.apply_edits(id, &[edit]).is_err() {
            return;
        }
        let new_end = old_end + line_break.len();
        let display = self.current().1.display();
        let selections = &mut self.windows[self.selected].selections;
        selections.update(|selection| {
            if selection.cursor == new_end {
                selection.cursor = old_end;
            }
            Ok::<(), ()>(())
        });
        // A `\n` after a lone `\r` makes one cluster of the two.
        clip_selections(selections, &display, &[], true);
    }

    /// The saves queued and not yet taken, each with the text it is to write.
    pub fn queued_saves(&self) -> impl Iterator<Item = &SaveJob> {
        self.save_jobs.iter()
    }

    /// The saves queued since the last call, for the host to run.
    pub fn take_save_jobs(&mut self) -> Vec<SaveJob> {
        std::mem::take(&mut self.save_jobs)
    }

    /// A save of `id` ended well: the disk holds the state `content`, and `stamp`
    /// describes the file. Edits since that state leave the buffer modified, and an
    /// exit refused while it ran then no longer speaks for what is at risk: an undo
    /// can make a buffer that looked unmodified differ from the disk only now.
    pub fn save_finished(&mut self, id: BufferId, content: ContentId, stamp: FileStamp) {
        let Some(buffer) = self.buffer_mut(id) else {
            return;
        };
        buffer.set_saving(false);
        buffer.mark_saved(content);
        buffer.set_written(stamp);
        buffer.set_changed_on_disk(false);
        buffer.set_gone_from_disk(false);
        if buffer.is_modified() {
            self.reset_exit_guard();
        }
    }

    /// A save of `id` found nothing to write: its file was there all along.
    pub fn save_unneeded(&mut self, id: BufferId) {
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_saving(false);
        }
    }

    /// A save of `id` wrote nothing, and the buffer stays modified. An exit refused
    /// while it ran no longer speaks for what is at risk, so it must be refused again.
    pub fn save_failed(&mut self, id: BufferId) {
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_saving(false);
            // A save over a file needs the one this failure found, if it found one, and
            // [`Editor::may_save_over`] says so after this.
            buffer.forget_save_over();
        }
        self.reset_exit_guard();
    }

    /// A save of `id` found its file changed, or now somewhere else. The modeline warns
    /// while the buffer has edits at stake, until a save gets through.
    pub fn file_changed_on_disk(&mut self, id: BufferId) {
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_changed_on_disk(true);
        }
    }

    /// The save of `id` that found its file changed found this regular file there, which a
    /// save the person agrees to may replace: [`Editor::start_save_over`].
    pub fn may_save_over(&mut self, id: BufferId, found: FileStamp) {
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_save_over(found);
        }
    }

    /// Whether an exit may go: at once if nothing is at risk, or once `confirmed`, if the
    /// person agreed to what was refused last and no more is at risk now. A save that failed
    /// or left its buffer modified since forgets the refusal, so it is asked about again.
    ///
    /// # Errors
    ///
    /// What is at risk, for a question to say. Its answer confirms.
    pub fn confirm_exit(&mut self, confirmed: bool) -> Result<(), ExitRefused> {
        let modified = self.buffers().filter(|(_, b)| b.is_modified()).count();
        let saving = self.buffers().filter(|(_, b)| b.is_saving()).count();
        // The person agreed to lose these buffers, and no other: a buffer modified since the
        // question asks again, even if another's save landed meanwhile.
        let at_risk: Vec<BufferId> = self
            .buffers()
            .filter(|(_, b)| b.is_modified() || b.is_saving())
            .map(|(id, _)| id)
            .collect();
        let agreed = self
            .exit_attempt
            .as_ref()
            .is_some_and(|asked| at_risk.iter().all(|id| asked.contains(id)));
        if at_risk.is_empty() || (confirmed && agreed) {
            return Ok(());
        }
        self.exit_attempt = Some(at_risk);
        Err(ExitRefused { modified, saving })
    }

    /// Forgets a refused exit, so that the next one is refused again.
    pub fn reset_exit_guard(&mut self) {
        self.exit_attempt = None;
    }
}
