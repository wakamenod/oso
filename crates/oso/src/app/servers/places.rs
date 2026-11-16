//! `M-.` and the jumps a server's answers lead to (ADR 0138, Go to definition and back, step 8),
//! on the UI thread: each answer of places, which goes to its place, or says why not; the asks
//! themselves are `asks`'. A file outside any project that an answer leads to
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
//!   places never open a float for the agent: it goes to the first. The person's several, and
//!   their references, are listed (`xrefs`, step 8b).
//! - A file joins by its buffer, once project detection has said it is in no project, and
//!   through the settling of opens, so ADR 0118 holds the agent's. One buffer joins one server;
//!   a server holds at most [`JOINED_MAX`] joined and [`JOINING_MAX`] waiting.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::VisitColumn;
use oso_core::syntax::Splice;
use oso_lsp::position::{Encoding, Position};
use oso_lsp::server::{AskFailure, AskKind, Location, Target};

use super::super::App;
use super::Running;
use super::asks::Asker;
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
    /// Goes to the answer's place, or lists its places for the person when it has several, or
    /// for `M-?`, one at once (step 8b); the agent goes to the first.
    pub(super) fn go_to_answer(
        &mut self,
        index: usize,
        (kind, asked, asker): (AskKind, BufferId, Asker),
        places: &[Location],
    ) {
        let Some(first) = places.first() else {
            let none = match kind {
                AskKind::Definition => "No definition found",
                AskKind::References => "No references found",
                // Completions never come here (`asks`), and a description goes to its float.
                AskKind::Completion | AskKind::Hover => return,
            };
            self.editor.message(none);
            return;
        };
        let Some(connection) = asker.agent() else {
            if places.len() > 1 {
                self.list_places(index, (kind, asked, asker), places);
                return;
            }
            // Said before the jump, as below.
            if kind == AskKind::References {
                self.editor.echo("1 reference");
            }
            self.person_jump(index, first);
            return;
        };
        // Said before the jump: said once a file not judged yet shows, it would go with it.
        if places.len() > 1 {
            self.editor.echo(format!("1 of {}", places.len()));
        }
        self.agent_jump(index, connection, first);
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
        let column = column(start.character as usize, encoding);
        let line = start.line as usize + 1;
        self.visit_place(Some(index), place.path.clone(), line, column);
        true
    }

    /// Visits `line`, from 1, and `column` of the file at `path` for the person, as `C-x C-f`
    /// would, keeping the place left; the file joins server `index` once known to be in no
    /// project, unless its name may hold secrets. An answer comes on a server's report, and the
    /// list's row is taken after its command: the visit starts here.
    pub(super) fn visit_place(
        &mut self,
        index: Option<usize>,
        path: PathBuf,
        line: usize,
        column: VisitColumn,
    ) {
        if let Some(index) = index
            && !Buffer::name_may_hold_secrets(&path)
        {
            self.join(index, path.clone());
        }
        self.editor
            .xref_visit(path, oso_core::load::UNASKED, line, column);
        self.serve_files();
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
    pub(super) fn open_place(&self, index: usize, place: &Location) -> Option<(BufferId, usize)> {
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
        // In the form a URI reads back as, as the place's path is.
        let read_back = buffer.path().and_then(super::read_back);
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

    /// [`App::open_place`] of each of `places`, each document looked up and its edits since read
    /// once, however many places it has.
    pub(super) fn open_places(
        &self,
        index: usize,
        places: &[Location],
    ) -> Vec<Option<(BufferId, usize)>> {
        type Found = Option<(BufferId, Option<PathBuf>, Option<Vec<Splice>>)>;
        let mut found: HashMap<(u64, u64), Found> = HashMap::new();
        let documents = self
            .servers
            .running
            .get(index)
            .map(|running| &running.documents);
        (places.iter())
            .map(|place| {
                let Target::Open {
                    document,
                    revision,
                    ref range,
                    ..
                } = place.target
                else {
                    return None;
                };
                let entry = found.entry((document, revision)).or_insert_with(|| {
                    let id = (documents?.keys().copied()).find(|id| id.get() == document)?;
                    let buffer = self.editor.buffer(id)?;
                    let read_back = buffer.path().and_then(super::read_back);
                    let splices = if buffer.revision() == revision {
                        None
                    } else {
                        Some(buffer.edits_since(revision)?)
                    };
                    Some((id, read_back, splices))
                });
                let (id, read_back, splices) = entry.as_ref()?;
                if read_back.as_deref() != Some(place.path.as_path()) {
                    return None;
                }
                let offset = splices
                    .as_ref()
                    .map_or(range.start, |splices| forward(range.start, splices));
                Some((*id, offset))
            })
            .collect()
    }

    /// Lets the file at `path` join server `index` once its buffer is known to be in no project:
    /// at once if it is, or when its project is found.
    pub(super) fn join(&mut self, index: usize, path: PathBuf) {
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
pub(super) fn line_and_column(target: &Target) -> (Position, Encoding) {
    match *target {
        Target::Lines {
            start, encoding, ..
        } => (start, encoding),
        Target::Open {
            start, encoding, ..
        } => (start, encoding),
    }
}

/// `column` as the visits count it, in the server's `encoding`.
pub(super) fn column(column: usize, encoding: Encoding) -> VisitColumn {
    match encoding {
        Encoding::Utf8 => VisitColumn::Utf8(column),
        Encoding::Utf16 => VisitColumn::Utf16(column),
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
