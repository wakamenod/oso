//! `M-.` and the jumps a server's answers lead to (ADR 0138, Go to definition and back, step 8),
//! on the UI thread: the ask, sent after the event's edits went to the server, and each answer,
//! which goes to its place, or says why not. A file outside any project that an answer leads to
//! joins the server that gave it, as `eglot-extend-to-xref` does, until its buffer is killed.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.
//!
//! - An answer goes only while the window that asked is selected and shows the buffer asked
//!   about: a person who moved on is not moved back.
//! - A target opens as `C-x C-f` would open it; a target whose name matches the secret patterns
//!   then opens as a buffer that may hold secrets, with no server.
//! - The agent's ask opens its target as the channel's `open` does: one whose name matches the
//!   secret patterns is refused, a file shows only once judged ordinary, it counts toward the
//!   connection's files and is the agent's, so its open with the server waits for the person
//!   (ADR 0118). Several places never open a float for the agent: it goes to the first.

use std::path::{Path, PathBuf};

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{At, Column};
use oso_core::syntax::Splice;
use oso_core::window::WindowId;
use oso_lsp::position::Encoding;
use oso_lsp::server::{Answer, Ask, AskKind, Location, State, Target};

use super::super::App;
use crate::channel::Connection;

/// Who asked, kept until the answer: the window to move, and the agent's connection.
#[derive(Debug, Clone, Copy)]
pub(super) struct Asker {
    window: WindowId,
    agent: Option<Connection>,
}

impl App {
    /// Asks the server following `at`'s buffer for `kind` there, or says why not.
    pub(super) fn lsp_find(&mut self, at: At, kind: AskKind) {
        let Some(buffer) = self.editor.buffer(at.buffer) else {
            return;
        };
        if buffer.is_secret() {
            self.editor
                .message("No server for a buffer that may hold secrets");
            return;
        }
        let followed = self.running_for(at.buffer).filter(|index| {
            self.servers.running[*index]
                .documents
                .get(&at.buffer)
                .is_some_and(|open| open.closed.is_none())
        });
        let Some(index) = followed else {
            self.editor
                .message("No language server follows this buffer");
            return;
        };
        let running = &self.servers.running[index];
        match running.state {
            State::Starting | State::Waiting => {
                let name = running.entry.name.clone();
                self.editor.message(format!("{name} is starting"));
                return;
            }
            State::Ready | State::Working(_) => {}
            State::Stopped | State::NotStarted | State::Ended => {
                let name = running.entry.name.clone();
                self.editor.message(format!("{name} does not run"));
                return;
            }
        }
        let agent = if at.agent {
            self.servers.asking_agent
        } else {
            None
        };
        running.server.ask(Ask {
            document: at.buffer.get(),
            kind,
            revision: at.revision,
            offset: at.offset,
        });
        let asker = Asker {
            window: self.editor.window().id(),
            agent,
        };
        self.servers.running[index]
            .asked
            .insert((at.buffer, kind), asker);
    }

    /// What server `index` answered.
    pub(super) fn lsp_answered(&mut self, index: usize, answer: Answer) {
        let Some(running) = self.servers.running.get_mut(index) else {
            return;
        };
        let Some(asked) = running
            .asked
            .keys()
            .map(|(id, _)| *id)
            .find(|id| id.get() == answer.document)
        else {
            return;
        };
        let Some(asker) = running.asked.remove(&(asked, answer.kind)) else {
            return;
        };
        let name = running.entry.name.clone();
        let window = self.editor.window();
        if window.id() != asker.window || window.buffer() != asked {
            return;
        }
        let places = match answer.outcome {
            Ok(places) => places,
            Err(why) => {
                // The server's text may quote what it read: shown, not logged.
                self.editor
                    .message_unlogged(format!("{name}: {} failed: {why}", answer.kind.noun()));
                self.request_redraw();
                return;
            }
        };
        let Some(first) = places.first() else {
            self.editor.message(match answer.kind {
                AskKind::Definition => "No definition found",
                AskKind::References => "No references found",
            });
            self.request_redraw();
            return;
        };
        let first = first.clone();
        self.join(index, &first.path);
        match asker.agent {
            Some(connection) => self.agent_jump(connection, &first),
            None => self.person_jump(&first),
        }
        if places.len() > 1 {
            self.editor.echo(format!("1 of {}", places.len()));
        }
        self.request_redraw();
    }

    /// Goes to `place` for the person.
    fn person_jump(&mut self, place: &Location) {
        if let Some((buffer, offset)) = self.open_place(place) {
            self.editor.jump_to_place(buffer, offset);
            return;
        }
        let Target::Lines {
            start, encoding, ..
        } = place.target
        else {
            return;
        };
        let column = match encoding {
            Encoding::Utf8 => Column::Utf8(start.character as usize),
            Encoding::Utf16 => Column::Utf16(start.character as usize),
        };
        self.editor.jump_visit(
            place.path.clone(),
            oso_core::load::UNASKED,
            start.line as usize + 1,
            column,
        );
    }

    /// Opens `place` for the agent's `connection`, as the channel's `open` does, at its line.
    fn agent_jump(&mut self, connection: Connection, place: &Location) {
        if Buffer::name_may_hold_secrets(&place.path) {
            self.agent_log("lsp-find-definition: refused, the name may hold secrets");
            self.editor
                .message("The definition's file may hold secrets: not opened for the agent");
            return;
        }
        let line = match &place.target {
            Target::Lines { start, .. } => start.line as usize + 1,
            Target::Open { .. } => match self.open_place(place) {
                Some((buffer, offset)) => self
                    .editor
                    .buffer(buffer)
                    .map_or(1, |buffer| buffer.text().offset_to_point(offset).row + 1),
                None => 1,
            },
        };
        self.editor.keep_jump_start();
        // The agent's reply went when its command ran: what the open says goes to `*Agent*`.
        let (reply, _) = std::sync::mpsc::channel();
        self.agent_open(connection, place.path.clone(), Some(line), reply);
    }

    /// The open buffer and offset of a place in a document open with the server, mapped
    /// through the buffer's edits since the text the server placed it in.
    fn open_place(&self, place: &Location) -> Option<(BufferId, usize)> {
        let Target::Open {
            document,
            revision,
            ref range,
        } = place.target
        else {
            return None;
        };
        let id = self
            .editor
            .buffers()
            .map(|(id, _)| id)
            .find(|id| id.get() == document)?;
        let buffer = self.editor.buffer(id)?;
        if buffer.path() != Some(place.path.as_path()) {
            return None;
        }
        let offset = if buffer.revision() == revision {
            range.start
        } else {
            match buffer.edits_since(revision) {
                Some(splices) => forward(range.start, splices),
                None => range.start,
            }
        };
        Some((id, offset))
    }

    /// Lets the file at `path` join server `index` if no project holds it, which the buffer's
    /// own project says once it is known: its buffer, now or once it loads, opens with the
    /// server ([`joins`]).
    fn join(&mut self, index: usize, path: &Path) {
        let Some(running) = self.servers.running.get_mut(index) else {
            return;
        };
        if running.joined.insert(path.to_owned()) {
            let open = self
                .editor
                .buffers()
                .find(|(_, buffer)| buffer.path() == Some(path))
                .map(|(id, _)| id);
            if let Some(id) = open {
                self.open_document(id);
                self.mark_servers();
            }
        }
    }
}

/// `offset` in a text moved through `splices`, in the old text's offsets: one inside a
/// replaced range goes to its start.
fn forward(offset: usize, mut splices: Vec<Splice>) -> usize {
    splices.sort_unstable_by_key(|splice| splice.start);
    let mut shift = 0isize;
    for splice in splices {
        if splice.start + splice.old_len <= offset {
            shift += splice.new_len as isize - splice.old_len as isize;
        } else if splice.start < offset {
            return splice.start.saturating_add_signed(shift);
        } else {
            break;
        }
    }
    offset.saturating_add_signed(shift)
}

/// Whether `buffer` joined the server whose answers led to `joined`: it is in no project, and
/// its file is one of them.
pub(super) fn joins(buffer: &Buffer, joined: &std::collections::HashSet<PathBuf>) -> bool {
    buffer.project().is_none() && buffer.path().is_some_and(|path| joined.contains(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offset_moves_through_the_splices_before_it() {
        let splice = |start, old_len, new_len| Splice {
            start,
            old_len,
            new_len,
        };
        let splices = vec![splice(20, 2, 0), splice(2, 0, 3)];
        assert_eq!(forward(10, splices.clone()), 13);
        assert_eq!(forward(1, splices.clone()), 1);
        assert_eq!(
            forward(21, splices.clone()),
            23,
            "inside a replaced range: its start"
        );
        assert_eq!(forward(30, splices), 31);
    }
}
