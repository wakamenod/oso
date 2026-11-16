//! `M-.` and the jumps a server's answers lead to (ADR 0138, Go to definition and back, step 8),
//! on the UI thread: the ask, sent after the event's edits went to the server, and each answer,
//! which goes to its place, or says why not. A file outside any project that an answer leads to
//! joins the server that gave it, as `eglot-extend-to-xref` does, until its buffer is killed.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.
//!
//! - An answer goes only while nothing moved since the ask: the window that asked is selected,
//!   shows the buffer asked about at the same revision with the primary cursor where it was,
//!   and no prompt or isearch is open. A person who moved on is not moved back.
//! - A target opens as `C-x C-f` would open it; a target whose name matches the secret patterns
//!   then opens as a buffer that may hold secrets, with no server, and joins nothing.
//! - The agent's ask opens its target as the channel's `open` does: one whose name matches the
//!   secret patterns is refused, a file shows only once judged ordinary, it counts toward the
//!   connection's files and is the agent's, so its open with the server waits for the person
//!   (ADR 0118). Its answer is dropped once the connection or the channel is gone. Several
//!   places never open a float for the agent: it goes to the first.
//! - A file joins by its buffer, once project detection has said it is in no project, and
//!   through the settling of opens, so ADR 0118 holds the agent's. One buffer joins one server;
//!   a server holds at most [`JOINED_MAX`] joined and [`JOINING_MAX`] waiting.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{AskAt, VisitColumn};
use oso_core::syntax::Splice;
use oso_core::window::WindowId;
use oso_lsp::position::{Encoding, Position};
use oso_lsp::server::{Answer, Answered, Ask, AskFailure, AskKind, Location, State, Target};

use super::super::App;
use super::Running;
use crate::channel::Connection;

/// What the echo area says of an ask of `kind` to the server `name` that got no answer, the
/// server named first: that it did not answer, as the ADR's bounds say, or why not.
pub(super) fn failed(name: &str, kind: AskKind, why: &AskFailure) -> String {
    match why {
        AskFailure::Overdue => format!("{name} did not answer"),
        AskFailure::Other(why) => format!("{name}: {} failed: {why}", kind.noun()),
    }
}

/// The most buffers outside any project that join one server.
pub(super) const JOINED_MAX: usize = 64;

/// The most files one server's answers led to that wait for their project to be known.
pub(super) const JOINING_MAX: usize = 16;

/// Who asked, and where, kept until the answer, which must bring its serial back.
#[derive(Debug, Clone, Copy)]
pub(super) struct Asker {
    serial: u64,
    window: WindowId,
    pub(super) revision: u64,
    pub(super) offset: usize,
    agent: Option<Connection>,
}

/// What joins a server: buffers, and the files of answers whose buffers are not known to be in
/// no project yet.
#[derive(Debug, Default)]
pub(super) struct Joined {
    buffers: HashSet<BufferId>,
    waiting: VecDeque<PathBuf>,
}

impl Joined {
    /// Whether buffer `id` joined.
    pub(super) fn has(&self, id: BufferId) -> bool {
        self.buffers.contains(&id)
    }

    /// Lets `id` go, as its document closed or it was killed.
    pub(super) fn forget(&mut self, id: BufferId) {
        self.buffers.remove(&id);
    }

    /// Keeps only the buffers `alive` says are there.
    pub(super) fn retain(&mut self, alive: impl Fn(BufferId) -> bool) {
        if !self.buffers.is_empty() {
            self.buffers.retain(|id| alive(*id));
        }
    }
}

/// Whether `buffer`, `id`, joined `running`'s server: it is in no project, and joined.
pub(super) fn joins(id: BufferId, buffer: &Buffer, running: &Running) -> bool {
    buffer.project().is_none() && running.joined.has(id)
}

impl App {
    /// Asks the server following `at`'s buffer for `kind` there, or says why not. `agent` is
    /// the connection whose command asked, if the agent's did.
    pub(super) fn lsp_ask(&mut self, at: AskAt, kind: AskKind, agent: Option<Connection>) {
        // An agent's ask whose connection is not known never passes as the person's.
        if at.agent && agent.is_none() {
            return;
        }
        let Some(index) = self.server_to_ask(at.buffer) else {
            return;
        };
        self.servers.serial += 1;
        let serial = self.servers.serial;
        let running = &self.servers.running[index];
        running.server.ask(Ask {
            serial,
            document: at.buffer.get(),
            kind,
            revision: at.revision,
            offset: at.offset,
        });
        let asker = Asker {
            serial,
            window: self.editor.window().id(),
            revision: at.revision,
            offset: at.offset,
            agent,
        };
        self.servers.running[index]
            .asked
            .insert((at.buffer, kind), asker);
    }

    /// The server to ask about `buffer`: one that follows it and runs. Otherwise the echo area
    /// says why not, and `None`.
    pub(super) fn server_to_ask(&mut self, buffer: BufferId) -> Option<usize> {
        let shown = self.editor.buffer(buffer)?;
        if shown.is_secret() {
            self.editor
                .message("No server for a buffer that may hold secrets");
            return None;
        }
        let followed = self.running_for(buffer).filter(|index| {
            self.servers.running[*index]
                .documents
                .get(&buffer)
                .is_some_and(|open| open.closed.is_none())
        });
        let Some(index) = followed else {
            self.editor
                .message("No language server follows this buffer");
            return None;
        };
        let running = &self.servers.running[index];
        let why = match running.state {
            State::Ready | State::Working(_) => return Some(index),
            State::Starting | State::Waiting => "is starting",
            State::Stopped | State::NotStarted | State::Ended => "does not run",
        };
        let name = running.entry.name.clone();
        self.editor.message(format!("{name} {why}"));
        None
    }

    /// What server `index` answered. The places go to the freeing thread, as up to 1000 paths
    /// are many to free on the UI thread.
    pub(super) fn lsp_answered(&mut self, index: usize, answer: Answer) {
        let Answer {
            serial,
            document,
            kind,
            outcome,
            ..
        } = answer;
        let taken = self.take_asker(index, serial, document, kind);
        // A description goes to its float (step 10).
        if kind == AskKind::Hover {
            self.lsp_described(index, taken, outcome);
            return;
        }
        let places = match outcome {
            Ok(Answered::Places(places)) => places,
            // A kind a later step asks for.
            Ok(_) => return,
            Err(why) => {
                if let Some((asked, asker)) = taken
                    && self.still_there(asked, &asker)
                {
                    let name = self.servers.running[index].entry.name.clone();
                    // The server's text may quote what it read: shown, not logged.
                    self.editor.message_unlogged(failed(&name, kind, &why));
                    self.request_redraw();
                }
                return;
            }
        };
        if let Some((asked, asker)) = taken
            && self.still_there(asked, &asker)
        {
            self.go_to_answer(index, kind, asker, &places);
            self.request_redraw();
        }
        self.freer.free(places);
    }

    /// Takes who asked server `index` the ask `kind` of the document `document`, if `serial`
    /// is that ask's: an answer to an earlier one, which a newer replaced, is no one's.
    pub(super) fn take_asker(
        &mut self,
        index: usize,
        serial: u64,
        document: u64,
        kind: AskKind,
    ) -> Option<(BufferId, Asker)> {
        let running = self.servers.running.get_mut(index)?;
        let asked = running
            .asked
            .keys()
            .map(|(id, _)| *id)
            .find(|id| id.get() == document)?;
        let key = (asked, kind);
        if running.asked.get(&key)?.serial != serial {
            return None;
        }
        Some((asked, running.asked.remove(&key)?))
    }

    /// Whether nothing moved since `asker` asked about `asked`, and an agent's connection is
    /// still let in.
    pub(super) fn still_there(&self, asked: BufferId, asker: &Asker) -> bool {
        let (window, buffer) = self.editor.current();
        window.id() == asker.window
            && window.buffer() == asked
            && buffer.revision() == asker.revision
            && window.selections.primary().cursor == asker.offset
            && self.editor.prompt().is_none()
            && self.editor.isearch().is_none()
            && asker
                .agent
                .is_none_or(|connection| self.agent_connected(connection))
    }

    fn go_to_answer(&mut self, index: usize, kind: AskKind, asker: Asker, places: &[Location]) {
        let Some(first) = places.first() else {
            let none = match kind {
                AskKind::Definition => "No definition found",
                AskKind::References => "No references found",
                // A description goes to its float, never here.
                AskKind::Hover => return,
            };
            self.editor.message(none);
            return;
        };
        // Said before the jump: said once a file not judged yet shows, it would go with it.
        if places.len() > 1 {
            self.editor.echo(format!("1 of {}", places.len()));
        }
        match asker.agent {
            Some(connection) => self.agent_jump(index, connection, first),
            None => self.person_jump(index, first),
        };
    }

    /// Goes to `place` for the person; whether it went.
    fn person_jump(&mut self, index: usize, place: &Location) -> bool {
        if let Some((buffer, offset)) = self.open_place(index, place) {
            self.editor.xref_to_place(buffer, offset);
            return true;
        }
        // A place in an open document whose buffer is gone, or no longer maps it, is visited
        // by the server's line and column.
        let (start, encoding) = line_and_column(&place.target);
        if !Buffer::name_may_hold_secrets(&place.path) {
            self.join(index, place.path.clone());
        }
        let column = match encoding {
            Encoding::Utf8 => VisitColumn::Utf8(start.character as usize),
            Encoding::Utf16 => VisitColumn::Utf16(start.character as usize),
        };
        self.editor.xref_visit(
            place.path.clone(),
            oso_core::load::UNASKED,
            start.line as usize + 1,
            column,
        );
        // An answer comes on a server's report, not after a command: the visit starts here.
        self.serve_files();
        true
    }

    /// Opens `place` for the agent's `connection`, as the channel's `open` does, at its line;
    /// whether it was let start.
    fn agent_jump(&mut self, index: usize, connection: Connection, place: &Location) -> bool {
        if Buffer::name_may_hold_secrets(&place.path) {
            self.agent_log("lsp-find-definition: refused, the name may hold secrets");
            self.editor
                .message("The definition's file may hold secrets: not opened for the agent");
            return false;
        }
        let line = match (&place.target, self.open_place(index, place)) {
            (Target::Open { .. }, Some((buffer, offset))) => self
                .editor
                .buffer(buffer)
                .map_or(1, |buffer| buffer.text().offset_to_point(offset).row + 1),
            (target, _) => line_and_column(target).0.line as usize + 1,
        };
        self.join(index, place.path.clone());
        // The agent's reply went when its command ran: what the open says goes to `*Agent*`.
        let (reply, said) = std::sync::mpsc::channel();
        self.agent_open(connection, place.path.clone(), Some(line), true, reply);
        if let Ok(Err(why)) = said.try_recv() {
            self.agent_log(&format!("lsp-find-definition: open refused: {why}"));
            return false;
        }
        true
    }

    /// The open buffer and offset of a place in a document open with server `index`, mapped
    /// through the buffer's edits since the text the server placed it in.
    fn open_place(&self, index: usize, place: &Location) -> Option<(BufferId, usize)> {
        let Target::Open {
            document,
            revision,
            ref range,
            ..
        } = place.target
        else {
            return None;
        };
        let id = self
            .servers
            .running
            .get(index)?
            .documents
            .keys()
            .copied()
            .find(|id| id.get() == document)?;
        let buffer = self.editor.buffer(id)?;
        // In the form a URI reads back as, as the place's path is: on Windows the drive letter
        // upper case and no `\\?\` prefix, however the buffer's path is written.
        let read_back = buffer
            .path()
            .and_then(oso_lsp::uri::from_path)
            .and_then(|uri| oso_lsp::uri::to_path(&uri));
        if read_back.as_deref() != Some(place.path.as_path()) {
            return None;
        }
        let offset = if buffer.revision() == revision {
            range.start
        } else {
            forward(range.start, &buffer.edits_since(revision)?)
        };
        Some((id, offset))
    }

    /// Lets the file at `path` join server `index` once its buffer is known to be in no project:
    /// at once if it is, or when its project is found.
    fn join(&mut self, index: usize, path: PathBuf) {
        let open = self
            .editor
            .buffers()
            .find(|(_, buffer)| buffer.path() == Some(path.as_path()))
            .map(|(id, buffer)| (id, buffer.project().is_some()));
        match open {
            Some((_, true)) => {}
            Some((id, false)) if self.servers.no_project.contains(&id) => {
                self.join_buffer(index, id)
            }
            _ => {
                let Some(running) = self.servers.running.get_mut(index) else {
                    return;
                };
                let waiting = &mut running.joined.waiting;
                if !waiting.contains(&path) {
                    if waiting.len() == JOINING_MAX {
                        waiting.pop_front();
                    }
                    waiting.push_back(path);
                }
            }
        }
    }

    /// Lets buffer `id`, in no project, join server `index`, unless one has it or the server
    /// has as many as it may, and settles its open again.
    fn join_buffer(&mut self, index: usize, id: BufferId) {
        let taken = self
            .servers
            .running
            .iter()
            .any(|running| running.joined.has(id));
        let Some(running) = self.servers.running.get_mut(index) else {
            return;
        };
        if taken || running.joined.buffers.len() >= JOINED_MAX {
            return;
        }
        running.joined.buffers.insert(id);
        self.unsettle_open(id);
    }

    /// Project detection answered for `ids`: those in no project are known so now, and each
    /// that an answer led to joins its server.
    pub(in crate::app) fn projects_known(&mut self, ids: &[BufferId]) {
        let mut joining = Vec::new();
        for &id in ids {
            let Some(buffer) = self.editor.buffer(id) else {
                self.servers.no_project.remove(&id);
                continue;
            };
            if buffer.project().is_some() || buffer.path().is_none() {
                self.servers.no_project.remove(&id);
                continue;
            }
            self.servers.no_project.insert(id);
            let path = buffer.path().map(PathBuf::from);
            for (index, running) in self.servers.running.iter_mut().enumerate() {
                let waiting = &mut running.joined.waiting;
                if let Some(at) = waiting.iter().position(|kept| Some(kept) == path.as_ref()) {
                    waiting.remove(at);
                    joining.push((index, id));
                }
            }
        }
        let editor = &self.editor;
        self.servers
            .no_project
            .retain(|id| editor.buffer(*id).is_some());
        for (index, id) in joining {
            self.join_buffer(index, id);
        }
    }

    /// The project of the server buffer `id` joined, if it joined one: its open is settled as
    /// that project's files' are, with that project's trust.
    pub(in crate::app) fn joined_project(
        &self,
        id: BufferId,
    ) -> Option<oso_core::project::ProjectId> {
        self.servers
            .running
            .iter()
            .find(|running| running.joined.has(id))
            .map(|running| running.project)
    }
}

/// Where `target` starts, as the server counts it, and in which encoding.
fn line_and_column(target: &Target) -> (Position, Encoding) {
    match *target {
        Target::Lines {
            start, encoding, ..
        } => (start, encoding),
        Target::Open {
            start, encoding, ..
        } => (start, encoding),
    }
}

/// `offset` in a text moved through `splices`, ascending, in the old text's offsets: one inside
/// a replaced range goes to its start.
fn forward(offset: usize, splices: &[Splice]) -> usize {
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
        let splices = [splice(2, 0, 3), splice(20, 2, 0)];
        assert_eq!(forward(10, &splices), 13);
        assert_eq!(forward(1, &splices), 1);
        assert_eq!(
            forward(21, &splices),
            23,
            "inside a replaced range: its start"
        );
        assert_eq!(forward(30, &splices), 31);
    }
}
