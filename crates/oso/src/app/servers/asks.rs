//! The asks of a server about a buffer (ADR 0138, steps 8, 9, 10 and 13), on the UI thread: each
//! sent after the event's edits went to the server, who asked and where kept until its answer,
//! and each answer routed by its kind: places to their jump or list (`places`, `xrefs`),
//! completions to the popup (`complete`), a description to its float (`hover`), and code
//! actions to their list and their edits (`actions`).
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.
//!
//! - An answer goes only while nothing moved since the ask: the window that asked is selected,
//!   shows the buffer asked about at the same revision with the primary cursor where it was,
//!   and no prompt or isearch is open. A person who moved on is not moved back.
//! - An answer to an earlier ask, which a newer of its kind replaced, is no one's.
//! - An agent's ask whose connection is not known never passes as the person's, and its
//!   answer is dropped once the connection or the channel is gone.

use oso_core::buffer::BufferId;
use oso_core::editor::AskAt;
use oso_core::window::WindowId;
use oso_lsp::server::{Answer, Answered, Ask, AskContext, AskKind, State};

use super::super::App;
use crate::channel::Connection;

/// Who asked, and where, kept until the answer, which must bring its serial back.
#[derive(Debug, Clone, Copy)]
pub(super) struct Asker {
    serial: u64,
    window: WindowId,
    revision: u64,
    offset: usize,
    who: Who,
}

/// Who asked: the person, the agent's connection whose command ran, or the host by itself, for
/// the person's typing, of which nothing is said when it finds nothing or fails (step 9b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Who {
    Person,
    Agent(Connection),
    Host,
}

impl Asker {
    /// The ask's serial, which numbers the list of its places too.
    pub(super) fn serial(&self) -> u64 {
        self.serial
    }

    /// Whether the host asked by itself, so that nothing is said of no answer.
    pub(super) fn quiet(&self) -> bool {
        self.who == Who::Host
    }

    /// The agent's connection whose command asked, if the agent's did.
    pub(super) fn agent(&self) -> Option<Connection> {
        match self.who {
            Who::Agent(connection) => Some(connection),
            Who::Person | Who::Host => None,
        }
    }

    /// Where it asked about `asked`, as the editor judges an answer against it.
    pub(super) fn at(&self, asked: BufferId) -> AskAt {
        AskAt {
            buffer: asked,
            revision: self.revision,
            offset: self.offset,
            agent: matches!(self.who, Who::Agent(_)),
        }
    }
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
        let who = agent.map_or(Who::Person, Who::Agent);
        // The person's `C-M-i` invokes completions; places take no context.
        let context = match kind {
            AskKind::Completion => AskContext::Completion(oso_lsp::protocol::Invoked::Invoked),
            // Code actions are asked with their range (`actions`); this never asks them.
            AskKind::Definition
            | AskKind::References
            | AskKind::Resolve
            | AskKind::Hover
            | AskKind::CodeAction
            | AskKind::ResolveAction
            | AskKind::ExecuteCommand
            | AskKind::PrepareRename
            | AskKind::Rename => AskContext::None,
        };
        self.send_ask(index, (at, kind, context), who);
    }

    /// Sends server `index` the ask of `kind` at `at` with `context`, and keeps `who` asked.
    pub(super) fn send_ask(
        &mut self,
        index: usize,
        (at, kind, context): (AskAt, AskKind, AskContext),
        who: Who,
    ) {
        self.send_ask_named(index, (at, kind, context), who, None);
    }

    /// [`App::send_ask`], with a rename's new name (step 14).
    pub(super) fn send_ask_named(
        &mut self,
        index: usize,
        (at, kind, context): (AskAt, AskKind, AskContext),
        who: Who,
        name: Option<Box<str>>,
    ) {
        // A newer ask for places stops the list that waits for its rows (step 8b).
        if matches!(kind, AskKind::Definition | AskKind::References)
            && let Some(old) = self.servers.xrefs.stop_waiting()
        {
            self.freer.free(old);
        }
        self.servers.serial += 1;
        let serial = self.servers.serial;
        let running = &self.servers.running[index];
        let ask = Ask {
            serial,
            document: at.buffer.get(),
            kind,
            revision: at.revision,
            offset: at.offset,
            context,
        };
        match name {
            Some(name) => running.server.rename(ask, name),
            None => running.server.ask(ask),
        }
        let asker = Asker {
            serial,
            window: self.editor.window().id(),
            revision: at.revision,
            offset: at.offset,
            who,
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
            Ok(Answered::Completion(completions)) => {
                match taken {
                    Some(taken) => self.completed(index, taken, completions),
                    None => self.freer.free(completions),
                }
                return;
            }
            Ok(Answered::Resolved(resolved)) => {
                if taken.is_some() {
                    self.resolved(resolved);
                }
                return;
            }
            Ok(Answered::CodeActions(actions)) => {
                self.code_actions_came(index, taken, actions);
                return;
            }
            Ok(Answered::ActionResolved(resolved)) => {
                self.code_action_resolved(index, taken, resolved);
                return;
            }
            // Its edits came while it waited (step 13b).
            Ok(Answered::Executed) => return,
            Ok(Answered::Prepared(prepared)) => {
                self.rename_checked(index, taken, prepared);
                self.step_rename();
                return;
            }
            Ok(Answered::Renamed(edit)) => {
                self.renamed(index, taken, edit);
                self.step_rename();
                return;
            }
            // A kind a later step asks for.
            Ok(_) => return,
            // A rename's failure ends it, wherever the person is now (step 14).
            Err(why) if matches!(kind, AskKind::PrepareRename | AskKind::Rename) => {
                self.rename_failed(index, taken, kind, &why);
                self.step_rename();
                return;
            }
            Err(why) => {
                if let Some((asked, asker)) = taken
                    && !asker.quiet()
                    && self.still_there(asked, &asker)
                {
                    let name = self.servers.running[index].entry.name.clone();
                    // The server's text may quote what it read: shown, not logged.
                    self.editor
                        .message_unlogged(super::places::failed(&name, kind, &why));
                    self.request_redraw();
                }
                return;
            }
        };
        if let Some((asked, asker)) = taken
            && self.still_there(asked, &asker)
        {
            self.go_to_answer(index, (kind, asked, asker), &places);
            self.request_redraw();
        }
        self.freer.free(places);
    }

    /// Takes who asked server `index` the ask `kind` of the document `document`, if `serial`
    /// is that ask's: an answer to an earlier one, which a newer replaced, is no one's.
    fn take_asker(
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
                .agent()
                .is_none_or(|connection| self.agent_connected(connection))
    }
}
