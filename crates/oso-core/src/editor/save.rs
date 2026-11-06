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
//! [`Editor::confirm_exit`] is the guard on exiting with work at stake. M2-16's prompt
//! replaces it.

use std::fmt;
use std::path::{Path, PathBuf};

use super::{Editor, clip_cursors};
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
        })
    }
}

/// A way of exiting. Only repeating the same one confirms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitAction {
    /// The exit command, by a key or by name.
    Command,
    /// The window's close button.
    CloseWindow,
}

/// What an exit would put at risk, which is why it was refused this time.
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
        f.write_str("; repeat to exit anyway")
    }
}

/// Why `buffer` cannot be saved now, if it cannot. Unmodified comes before no file,
/// as in Emacs, where `C-x C-s` in an untouched `*scratch*` has nothing to do.
fn refusal(buffer: &Buffer) -> Option<SaveRefused> {
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
fn missing_final_newline(buffer: &Buffer) -> Option<&'static str> {
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
        let id = self.window().buffer();
        let buffer = self.current().1;
        if let Some(refused) = refusal(buffer) {
            return Err(refused);
        }
        // An unmodified buffer stays as it is unless the file has gone, which only the
        // host can tell, and then it holds what the file held.
        let only_if_gone = !buffer.is_modified();
        let line_break = missing_final_newline(buffer).filter(|_| !only_if_gone);
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
                baseline: buffer.baseline().cloned(),
                only_if_gone,
            };
            buffer.set_saving(true);
            self.save_jobs.push(job);
        }
        Ok(line_break.is_some())
    }

    /// Adds `line_break` at the end of `id`, the selected window's buffer, and
    /// keeps the cursors that were at the end where they were.
    fn insert_final_newline(&mut self, id: BufferId, line_break: &str) {
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
        clip_cursors(selections, &display, &[]);
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

    /// Whether `action` may exit: at once if nothing is at risk, or if it is the
    /// action refused last and no other key came since.
    ///
    /// # Errors
    ///
    /// What is at risk, the first time. Repeating `action` then confirms.
    pub fn confirm_exit(&mut self, action: ExitAction) -> Result<(), ExitRefused> {
        let modified = self.buffers().filter(|(_, b)| b.is_modified()).count();
        let saving = self.buffers().filter(|(_, b)| b.is_saving()).count();
        if modified + saving == 0 || self.exit_attempt == Some(action) {
            return Ok(());
        }
        self.exit_attempt = Some(action);
        Err(ExitRefused { modified, saving })
    }

    /// The way of exiting that was last refused, if no other key came since.
    pub fn exit_attempt(&self) -> Option<ExitAction> {
        self.exit_attempt
    }

    /// Forgets a refused exit, so that the next one is refused again.
    pub fn reset_exit_guard(&mut self) {
        self.exit_attempt = None;
    }
}
