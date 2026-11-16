//! The documents the app keeps in step with each language server (ADR 0138, Document sync,
//! step 4), on the UI thread: which buffer may be opened with its server, opening it, handing
//! over each batch of edits after an event, closing it when its buffer goes or no longer fits,
//! and the person's saves. A batch is handed over as the text last sent, the text now, both
//! O(1) clones, and the splices between them from the buffer's log; the session thread works
//! out the protocol's changes.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.

use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId, Mode, ReadOnly};
use oso_core::rope::Rope;
use oso_lsp::limits::DOCUMENT_BYTES_MAX;
use oso_lsp::server::{Change, Closed, Document};

use super::{App, Running};

/// A document the app opened with a server.
#[derive(Debug)]
pub(super) struct Open {
    /// The file it is, as it was opened: a buffer written elsewhere is another document.
    path: PathBuf,
    mode: Mode,
    /// The revision and text last handed over.
    revision: u64,
    text: Rope,
    /// Kept from the server, and why, as its thread reported.
    pub(super) closed: Option<Closed>,
}

/// The language a buffer of `mode` holds, as the protocol names it.
fn language_id(mode: Mode) -> &'static str {
    match mode {
        Mode::Rust => "rust",
        Mode::Markdown => "markdown",
        Mode::Toml => "toml",
        _ => "plaintext",
    }
}

/// Whether `buffer` may be opened with `running`'s server: a file's buffer, loaded and judged,
/// not one that may hold secrets, not read-only for its bytes, not internal or a terminal, in
/// the server's project and mode, and within the bound. Its URI, if so.
fn fits(buffer: &Buffer, running: &Running) -> Option<String> {
    let path = buffer.path()?;
    let fits = buffer.loading().is_none()
        && !buffer.is_secret()
        && !matches!(
            buffer.read_only(),
            Some(
                ReadOnly::InvalidUtf8 | ReadOnly::Binary | ReadOnly::Internal | ReadOnly::Terminal
            )
        )
        && !buffer.is_internal()
        && !buffer.is_terminal()
        && buffer.project() == Some(running.project)
        && running.entry.modes.contains(&buffer.mode())
        && buffer.text().len() <= DOCUMENT_BYTES_MAX;
    if !fits {
        return None;
    }
    oso_lsp::uri::from_path(path)
}

/// Whether the agent opened `buffer`, and the person has not let its open go (ADR 0118): only
/// its own settling opens it with a server.
fn the_agents(buffer: &Buffer) -> bool {
    buffer.opened_by_agent() && !buffer.open_let_go()
}

impl App {
    /// The server running for `id`'s project and mode, by its place.
    fn running_for(&self, id: BufferId) -> Option<usize> {
        let buffer = self.editor.buffer(id)?;
        let project = buffer.project()?;
        self.servers.running.iter().position(|running| {
            running.project == project && running.entry.modes.contains(&buffer.mode())
        })
    }

    /// Whether opening `id` would open it with a server that runs or would start: it fits, and
    /// no server has it yet.
    pub(in crate::app) fn would_open_document(&self, id: BufferId) -> bool {
        let Some(buffer) = self.editor.buffer(id) else {
            return false;
        };
        match self.running_for(id) {
            Some(index) => {
                let running = &self.servers.running[index];
                !running.stopping
                    && !running.documents.contains_key(&id)
                    && fits(buffer, running).is_some()
            }
            None => self.would_start_server(id),
        }
    }

    /// Whether a save of `id` would tell a server: one has its document.
    pub(in crate::app) fn would_tell_a_save(&self, id: BufferId) -> bool {
        self.running_for(id)
            .is_some_and(|index| self.servers.running[index].documents.contains_key(&id))
    }

    /// Opens `id` with its server, if it fits and the server has it not.
    pub(in crate::app) fn open_document(&mut self, id: BufferId) {
        let Some(index) = self.running_for(id) else {
            return;
        };
        self.open_with(index, id);
    }

    /// Opens with server `index` every buffer that fits and is not the agent's, as when it
    /// starts.
    pub(super) fn open_documents_of(&mut self, index: usize) {
        let ids: Vec<BufferId> = self
            .editor
            .buffers()
            .filter(|(_, buffer)| !the_agents(buffer) && buffer.open_waits().is_empty())
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            self.open_with(index, id);
        }
    }

    fn open_with(&mut self, index: usize, id: BufferId) {
        let Some(running) = self.servers.running.get(index) else {
            return;
        };
        if running.stopping || running.documents.contains_key(&id) {
            return;
        }
        let Some(buffer) = self.editor.buffer(id) else {
            return;
        };
        let Some(uri) = fits(buffer, running) else {
            return;
        };
        let (path, mode) = (
            buffer.path().map(PathBuf::from).unwrap_or_default(),
            buffer.mode(),
        );
        let text = buffer.text().clone();
        let revision = buffer.revision();
        running.server.open(Document {
            id: id.get(),
            uri,
            language_id: language_id(mode).to_owned(),
            text: text.clone(),
        });
        self.servers.running[index].documents.insert(
            id,
            Open {
                path,
                mode,
                revision,
                text,
                closed: None,
            },
        );
        if let Some(buffer) = self.editor.buffer_mut(id) {
            buffer.set_followed(true);
        }
    }

    /// The person's save of `id` reached the server's thread's turn: `didSave`, or an open again
    /// of a document kept from the server whose reason went (step 4's Q2).
    pub(in crate::app) fn tell_a_save(&mut self, id: BufferId) {
        if let Some(index) = self.running_for(id)
            && self.servers.running[index].documents.contains_key(&id)
        {
            self.servers.running[index].server.save(id.get());
        }
    }

    /// After each event: a document whose buffer went, or no longer fits, as after `C-x C-w`
    /// or a change of mode, is closed, and one written to a file that still fits is opened as
    /// that file; each other whose text moved gets the batch since.
    pub(super) fn sync_documents(&mut self) {
        let mut reopen = Vec::new();
        let mut unfollow = Vec::new();
        for (index, running) in self.servers.running.iter_mut().enumerate() {
            let editor = &self.editor;
            let mut gone = Vec::new();
            for (&id, open) in &mut running.documents {
                let Some(buffer) = editor.buffer(id) else {
                    gone.push(id);
                    continue;
                };
                if buffer.path() != Some(open.path.as_path()) || buffer.mode() != open.mode {
                    // Another file, or another mode: the old document goes, and the buffer is
                    // opened again as it is now, if it fits.
                    gone.push(id);
                    reopen.push((index, id));
                    continue;
                }
                if no_longer_fits(buffer, running.project, &running.entry.modes) {
                    gone.push(id);
                    continue;
                }
                if buffer.revision() == open.revision {
                    continue;
                }
                let after = buffer.text().clone();
                let splices = buffer.edits_since(open.revision);
                running.server.change(Change {
                    id: id.get(),
                    before: std::mem::replace(&mut open.text, after.clone()),
                    after,
                    splices,
                });
                open.revision = buffer.revision();
            }
            for id in gone {
                running.documents.remove(&id);
                running.server.close(id.get());
                unfollow.push(id);
            }
        }
        self.unfollow(unfollow);
        for (index, id) in reopen {
            self.open_with(index, id);
        }
    }

    /// Stops following the buffers of a server that ended.
    pub(super) fn unfollow(&mut self, documents: impl IntoIterator<Item = BufferId>) {
        for id in documents {
            if let Some(buffer) = self.editor.buffer_mut(id) {
                buffer.set_followed(false);
            }
        }
    }
}

/// Whether `buffer`, whose document a server has, no longer fits it: it may hold secrets now,
/// is read-only for its bytes, or left the server's project or modes. Its size is the server
/// thread's to judge, as it grows and shrinks.
fn no_longer_fits(buffer: &Buffer, project: oso_core::project::ProjectId, modes: &[Mode]) -> bool {
    buffer.is_secret()
        || matches!(
            buffer.read_only(),
            Some(ReadOnly::InvalidUtf8 | ReadOnly::Binary)
        )
        || buffer.project() != Some(project)
        || !modes.contains(&buffer.mode())
}
