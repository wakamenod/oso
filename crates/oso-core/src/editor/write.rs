//! Writing a buffer under a new name, `C-x C-w` (ADR 0087): the checks and the job, the file
//! found there that a person may agree to write over, and the buffer moving to its new file.
//!
//! Must not depend on: the host's window, threads or I/O; the host writes and reports back.

use std::path::{Path, PathBuf};

use super::Editor;
use super::save::{Elsewhere, Over, SaveJob, SaveRefused, missing_final_newline};
use super::trim::{Then, Trimmed};
use crate::buffer::{Baseline, Buffer, BufferId, ContentId, FileStamp, ReadOnly};

/// A file a write to a new place found, which `y` may write over, and only for that buffer
/// and that path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WriteOver {
    buffer: BufferId,
    path: PathBuf,
    over: Over,
}

impl Editor {
    /// Starts writing the selected window's buffer to `path`, which it visits once the write
    /// is done: its final newline first, as a save adds it, then a job for the host. A write to
    /// the path the buffer visits is a save. With `over`, it writes over the file a write found
    /// at `path` before, if it is still that file. Returns whether the text changed.
    ///
    /// # Errors
    ///
    /// Why the write did not start, `Exists` if a write found a file at `path` and the person
    /// has yet to agree. Nothing changes.
    pub fn start_write(&mut self, path: PathBuf, over: bool) -> Result<bool, SaveRefused> {
        let id = self.window().buffer();
        let buffer = self.current().1;
        if buffer.path() == Some(path.as_path()) {
            return self.queue_save(over);
        }
        // A prompt's text, a passphrase's among them, never goes to a file.
        if buffer.is_internal() {
            return Err(SaveRefused::ReadOnly(ReadOnly::Internal));
        }
        if let Some(reason) = buffer.read_only() {
            return Err(SaveRefused::ReadOnly(reason));
        }
        let open: Vec<PathBuf> = self
            .buffers()
            .filter(|(other, _)| *other != id)
            .filter_map(|(_, other)| {
                let place = other.baseline().map(|baseline| baseline.location.clone());
                Some((other.path()?.to_path_buf(), place))
            })
            .flat_map(|(path, place)| std::iter::once(path).chain(place))
            .collect();
        if open.contains(&path) {
            return Err(SaveRefused::OpenElsewhere);
        }
        if buffer.is_saving() {
            return Err(SaveRefused::AlreadySaving);
        }
        let asked = self
            .write_over_asked
            .take()
            .filter(|asked| asked.buffer == id && asked.path == path);
        let found = match asked {
            Some(asked) if over => Some(asked.over),
            Some(asked) => {
                self.write_over_asked = Some(asked);
                return Err(SaveRefused::Exists);
            }
            None => None,
        };
        let buffer = self.current().1;
        let name = buffer.path().and_then(Path::file_name).map_or_else(
            || buffer.name().to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        // A path ending in `/` names a directory, which takes that name.
        let path = match path.to_str().and_then(|text| text.strip_suffix('/')) {
            Some(directory) if !directory.is_empty() => PathBuf::from(directory).join(&name),
            _ => path,
        };
        let elsewhere = Elsewhere {
            over: found,
            open,
            name,
        };
        let then = Then::Write {
            path: path.clone(),
            elsewhere: elsewhere.clone(),
        };
        let (trimmed, kept) = match self.trim_for_save(id, then) {
            Trimmed::Done { changed, kept } => (changed, kept),
            Trimmed::Waiting => return Ok(false),
        };
        let newline = self.write_after_trim(id, path, elsewhere, kept)?;
        Ok(trimmed || newline)
    }

    /// Goes on with a write of `id` to `path` that may have waited for whitespace cleanup:
    /// its final newline, then its job, which takes what was edited but for `kept`. Returns
    /// whether the newline went in.
    pub(super) fn write_after_trim(
        &mut self,
        id: BufferId,
        path: PathBuf,
        elsewhere: Elsewhere,
        kept: Vec<std::ops::Range<usize>>,
    ) -> Result<bool, SaveRefused> {
        let buffer = self.buffer(id).ok_or(SaveRefused::NoFile)?;
        if buffer.is_internal() {
            return Err(SaveRefused::ReadOnly(ReadOnly::Internal));
        }
        if let Some(reason) = buffer.read_only() {
            return Err(SaveRefused::ReadOnly(reason));
        }
        if buffer.is_saving() {
            return Err(SaveRefused::AlreadySaving);
        }
        let line_break = missing_final_newline(buffer);
        if let Some(line_break) = line_break {
            self.insert_final_newline(id, line_break);
        }
        if let Some(buffer) = self.buffer_mut(id) {
            let job = SaveJob {
                buffer: id,
                path,
                text: buffer.text().clone(),
                bom: buffer.file_info().bom,
                content: buffer.content_id(),
                baseline: None,
                only_if_gone: false,
                elsewhere: Some(elsewhere),
            };
            buffer.set_saving(true);
            buffer.save_took_edits(kept);
            self.save_jobs.push(job);
        }
        Ok(line_break.is_some())
    }

    /// A write of `id` to `path` found the regular file `found` there, at `location`, and wrote
    /// nothing. The next `C-x C-w` of that path asks, and `y` writes over that file only, where
    /// the path led then.
    pub fn may_write_over(
        &mut self,
        id: BufferId,
        path: PathBuf,
        found: FileStamp,
        location: PathBuf,
    ) {
        self.write_over_asked = Some(WriteOver {
            buffer: id,
            path,
            over: Over { found, location },
        });
    }

    /// The path a write found a file at, which the next `C-x C-w` of it asks about.
    pub fn write_over_asked(&self) -> Option<&Path> {
        self.write_over_asked
            .as_ref()
            .map(|asked| asked.path.as_path())
    }

    /// A write of `id` to `path` put `content` there as `stamp`, at `location`: the buffer now
    /// visits it, unmodified, and is named after it. `judge` judges the new path, as a load
    /// does; a verdict that withheld the text keeps it withheld, wherever it was written
    /// (ADR 0087).
    pub fn written_elsewhere(
        &mut self,
        id: BufferId,
        path: PathBuf,
        content: ContentId,
        location: PathBuf,
        stamp: FileStamp,
        judge: impl FnOnce(&mut Buffer),
    ) {
        let Some(buffer) = self.buffer_mut(id) else {
            return;
        };
        let withheld = buffer.verdict().is_withheld();
        let mode_before = buffer.mode();
        let old = buffer
            .path()
            .map_or_else(|| buffer.name().to_owned(), crate::buffer::file_base);
        let new = crate::buffer::file_base(&path);
        buffer.set_saving(false);
        // What the write took is on disk: those lines are not edited any more.
        buffer.save_ended_edits(true);
        buffer.set_path(path);
        buffer.set_baseline(Baseline {
            location,
            file: Some(stamp),
        });
        buffer.mark_saved(content);
        judge(buffer);
        if withheld {
            buffer.treat_as_secret();
        }
        let mode = buffer.mode();
        if buffer.is_modified() {
            self.reset_exit_guard();
        }
        // A new name can change the mode: the buffer takes its indentation too (ADR 0115).
        if mode != mode_before {
            self.give_mode_indent(id);
        }
        // The buffers named like the old file may take a shorter name, and those like the new
        // one a longer one.
        self.name_files_like(&old);
        self.name_files_like(&new);
        // It may be in another project now, under other EditorConfig files (ADR 0115).
        self.request_project(id);
        self.request_editorconfig(id);
    }
}

#[cfg(test)]
mod tests;
