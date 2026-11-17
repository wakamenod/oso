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

/// The language a buffer of `mode` visiting `path` holds, as the protocol names it: JSX is
/// JavaScript's mode, and named by its file. A match with no wildcard, so that a new mode says.
fn language_id(mode: Mode, path: &std::path::Path) -> &'static str {
    let jsx = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jsx"));
    match mode {
        Mode::JavaScript if jsx => "javascriptreact",
        Mode::JavaScript => "javascript",
        Mode::Rust => "rust",
        Mode::Markdown => "markdown",
        Mode::Toml => "toml",
        Mode::Go => "go",
        Mode::TypeScript => "typescript",
        Mode::Tsx => "typescriptreact",
        Mode::Latex => "latex",
        Mode::Swift => "swift",
        Mode::Fundamental
        | Mode::CopyList
        | Mode::CopyView
        | Mode::Sessions
        | Mode::Brief
        | Mode::Worktrees
        | Mode::Review
        | Mode::Terminal
        | Mode::Directory
        | Mode::GitStatus
        | Mode::GitGraph
        | Mode::GitPr
        | Mode::GitPrList
        | Mode::GitIssueList
        | Mode::GitIssue
        | Mode::GitCommit
        | Mode::GitPrMessage
        | Mode::ReviewMessage
        | Mode::GitLog
        | Mode::Conversation
        | Mode::GitDiscarded => "plaintext",
    }
}

/// Whether `buffer` may be sent to a server at all: a file's buffer, loaded and judged, not one
/// that may hold secrets, not read-only for its bytes, not internal or a terminal. Its size is
/// the server's thread's to judge, which keeps it from the server and lets a save open it again.
fn may_be_sent(buffer: &Buffer) -> bool {
    buffer.path().is_some()
        && buffer.loading().is_none()
        && !buffer.is_secret()
        && !matches!(
            buffer.read_only(),
            Some(
                ReadOnly::InvalidUtf8 | ReadOnly::Binary | ReadOnly::Internal | ReadOnly::Terminal
            )
        )
        && !buffer.is_internal()
        && !buffer.is_terminal()
}

/// Whether `buffer` may be opened with `running`'s server: it may be sent, and is in the
/// server's project and mode. Its URI, if so.
fn fits(id: BufferId, buffer: &Buffer, running: &Running) -> Option<String> {
    let fits = may_be_sent(buffer)
        && belongs(id, buffer, running)
        && running.entry.modes.contains(&buffer.mode());
    if !fits {
        return None;
    }
    oso_lsp::uri::from_path(buffer.path()?)
}

/// Whether `buffer`, `id`, is the server's to have, by where its file is: in its project, or
/// outside any and joined to it by a jump (step 8).
fn belongs(id: BufferId, buffer: &Buffer, running: &Running) -> bool {
    buffer.project() == Some(running.project) || super::places::joins(id, buffer, running)
}

/// Whether the agent opened `buffer`, and the person has not let its open go (ADR 0118): only
/// its own settling opens it with a server.
fn the_agents(buffer: &Buffer) -> bool {
    buffer.opened_by_agent() && !buffer.open_let_go()
}

impl App {
    /// The server running for `id`'s project and mode, or that it joined, by its place.
    pub(super) fn running_for(&self, id: BufferId) -> Option<usize> {
        let buffer = self.editor.buffer(id)?;
        self.servers.running.iter().position(|running| {
            belongs(id, buffer, running) && running.entry.modes.contains(&buffer.mode())
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
                // A server that never started, as one not installed, holds nothing for the
                // person: `lsp-restart` opens what fits once it starts (step 5's Q2).
                !running.stopping
                    && running.state != oso_lsp::server::State::NotStarted
                    && !running.documents.contains_key(&id)
                    && fits(id, buffer, running).is_some()
            }
            None => may_be_sent(buffer) && self.would_start_server(id),
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

    /// Opens with server `index` every buffer that fits, is not the agent's, and has the mode
    /// its file's name gives it, as when it starts: a mode set since, which the agent can do,
    /// opens nothing the person did not visit as such.
    pub(super) fn open_documents_of(&mut self, index: usize) {
        let ids: Vec<BufferId> = self
            .editor
            .buffers()
            .filter(|(_, buffer)| {
                !the_agents(buffer)
                    && buffer.open_waits().is_empty()
                    && buffer
                        .path()
                        .is_some_and(|path| Mode::for_path(path) == buffer.mode())
            })
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
        let Some(uri) = fits(id, buffer, running) else {
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
            language_id: language_id(mode, &path).to_owned(),
            text: text.clone(),
            revision,
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

    /// After each event: a document whose buffer went, or no longer fits, as once it may hold
    /// secrets or is set to another mode, is closed; one written to another file is closed and
    /// its open settled again, as a new visit's is; each other whose text moved gets the batch
    /// since.
    /// One pass over the buffers for each server with documents. Returns whether a document
    /// closed, for the modeline.
    pub(super) fn sync_documents(&mut self) -> bool {
        let mut resettle = Vec::new();
        let mut unfollow = Vec::new();
        let editor = &self.editor;
        for running in &mut self.servers.running {
            // A buffer that joined leaves with its document, or once killed (step 8).
            running.joined.retain(|id| editor.buffer(id).is_some());
            if running.documents.is_empty() {
                continue;
            }
            let mut gone = Vec::new();
            let mut seen = 0;
            for (id, buffer) in editor.buffers() {
                let Some(open) = running.documents.get_mut(&id) else {
                    continue;
                };
                seen += 1;
                let belongs = buffer.project() == Some(running.project)
                    || (buffer.project().is_none() && running.joined.has(id));
                if buffer.path() != Some(open.path.as_path()) {
                    // Another file, as after the person's `C-x C-w`: its open is settled again
                    // once its project is known, as a new visit's is.
                    gone.push(id);
                    resettle.push(id);
                    continue;
                }
                if buffer.mode() != open.mode {
                    // Another mode, which the agent can set: closed, and opened again only by
                    // a visit or `lsp-start`.
                    gone.push(id);
                    continue;
                }
                if no_longer_fits(buffer, belongs, &running.entry.modes) {
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
                    revision: buffer.revision(),
                });
                open.revision = buffer.revision();
            }
            // Killed buffers, which the pass did not meet: looked for only when there are some.
            if seen < running.documents.len() {
                gone.extend(
                    running
                        .documents
                        .keys()
                        .filter(|id| editor.buffer(**id).is_none())
                        .copied(),
                );
            }
            for id in gone {
                running.documents.remove(&id);
                running.joined.forget(id);
                running.server.close(id.get());
                unfollow.push(id);
            }
        }
        let closed = !unfollow.is_empty();
        self.unfollow(unfollow);
        for id in resettle {
            self.unsettle_open(id);
        }
        closed
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
/// is read-only for its bytes, or left the server's project, `belongs` false, or modes. Its
/// size is the server thread's to judge, as it grows and shrinks.
fn no_longer_fits(buffer: &Buffer, belongs: bool, modes: &[Mode]) -> bool {
    buffer.is_secret()
        || matches!(
            buffer.read_only(),
            Some(ReadOnly::InvalidUtf8 | ReadOnly::Binary)
        )
        || !belongs
        || !modes.contains(&buffer.mode())
}
