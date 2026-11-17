//! A server's code actions on the UI thread (ADR 0138, step 13, as `eglot-code-actions`): the
//! person's ask over the region or the cursor's place, the list its answer opens, and the
//! picked action: its edit, resolved first if the server gives it only then, applied through
//! `applying`, then its command run.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.
//!
//! - Only the person asks (Q9): the commands are `Person`, so the agent channel never runs
//!   them, and the list is a float, which the agent never reads.
//! - The answer's actions are kept, with the server named by its root and entry, until one is
//!   picked or a newer list opens; the picked one keeps only what its resolve and its command
//!   need. What is let go is freed off the UI thread.
//! - An edit applies whole or not at all, to buffers still at the text the server had (Q8). An
//!   action's command runs after its edit applied, only one the server declared (Q5), asked at
//!   the next sync, once the edit's text went to the server.
//! - An action's title is the server's text, with its bidi and invisible characters written out:
//!   the echo area shows it after what changed, and no log has it.

use std::path::PathBuf;

use oso_core::buffer::BufferId;
use oso_core::editor::{AskAt, CodeActionItem};
use oso_lsp::server::{ActionResolved, AskContext, AskKind, CodeAction, CodeActions};

use super::super::App;
use super::applying::EditFrom;
use super::asks::{Asker, Who};

/// The actions of the list open now: the server that gave them, by its root and entry, the
/// buffer asked about and its revision, and the ask's serial, which a resolve names.
#[derive(Debug)]
pub(super) struct KeptActions {
    server: (PathBuf, String),
    buffer: BufferId,
    revision: u64,
    serial: u64,
    actions: Vec<CodeAction>,
}

/// The action picked from a list, while it resolves or until its command runs: its server, the
/// buffer asked about, the action by its answer's serial and its place, and its title.
#[derive(Debug, Clone)]
pub(super) struct Picked {
    pub(super) server: (PathBuf, String),
    buffer: BufferId,
    serial: u64,
    index: usize,
    pub(super) title: String,
}

/// What the echo area says of `title`, `server`'s action: `said`, then the title, last, so
/// that no title passes for what changed.
pub(super) fn said_of(server: &str, said: &str, title: &str) -> String {
    format!("{server}: {said}: \u{201c}{title}\u{201d}")
}

impl App {
    /// The person's `C-c C-r`: asks the server following `at`'s buffer for the code actions
    /// from `start` to `end`, or says why not.
    pub(super) fn lsp_code_actions(&mut self, at: AskAt, (start, end): (usize, usize)) {
        // Never the agent's: the commands are `Person`.
        if at.agent {
            return;
        }
        let Some(index) = self.server_to_ask(at.buffer) else {
            return;
        };
        let context = AskContext::Range { start, end };
        self.send_ask(index, (at, AskKind::CodeAction, context), Who::Person);
    }

    /// Server `index` answered `actions` for who asked, `taken`: listed, if nothing moved since.
    pub(super) fn code_actions_came(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        actions: CodeActions,
    ) {
        let Some((asked, asker)) = taken.filter(|(asked, asker)| self.still_there(*asked, asker))
        else {
            self.freer.free(actions);
            return;
        };
        let running = &self.servers.running[index];
        let server = (running.root.clone(), running.entry.name.clone());
        if actions.actions.is_empty() {
            self.editor
                .message(format!("{}: no code actions here", server.1));
            self.request_redraw();
            return;
        }
        let items: Vec<CodeActionItem> = (actions.actions.iter())
            .map(|action| CodeActionItem {
                title: action.title.clone(),
                kind: action.kind.clone(),
                preferred: action.preferred,
                disabled: action.disabled.clone(),
            })
            .collect();
        let opened = self.dispatcher.list_code_actions(&mut self.editor, &items);
        if opened.is_none() {
            self.freer.free(actions);
            return;
        }
        let kept = KeptActions {
            server,
            buffer: asked,
            revision: asker.at(asked).revision,
            serial: asker.serial(),
            actions: actions.actions,
        };
        if let Some(old) = self.servers.actions.replace(kept) {
            self.freer.free(old);
        }
        self.servers.resolving = None;
        self.request_redraw();
    }

    /// `lsp-code-action-apply`: the action the list took, applied, or resolved first.
    pub(super) fn apply_code_action(&mut self) {
        let (Some(hit), Some(mut kept)) = (
            self.editor.take_code_action_hit(),
            self.servers.actions.take(),
        ) else {
            return;
        };
        if hit >= kept.actions.len() {
            self.freer.free(kept);
            return;
        }
        let action = kept.actions.swap_remove(hit);
        let picked = Picked {
            server: kept.server.clone(),
            buffer: kept.buffer,
            serial: kept.serial,
            index: hit,
            title: action.title,
        };
        let revision = kept.revision;
        self.freer.free(kept);
        let name = picked.server.1.clone();
        let then = action.command.is_some().then(|| picked.clone());
        let now = self
            .editor
            .buffer(picked.buffer)
            .map(oso_core::buffer::Buffer::revision);
        if now != Some(revision) {
            let said = said_of(
                &name,
                "the buffer changed since the list opened",
                &picked.title,
            );
            self.editor.message_unlogged(said);
            self.request_redraw();
            return;
        }
        // An edit goes first, then the command it runs, as the protocol orders them.
        match action.edit {
            Some(Ok(edit)) => {
                let (server, title) = (picked.server.clone(), picked.title.clone());
                self.apply_edit_of(&server, &title, edit, EditFrom::Action(then));
            }
            Some(Err(why)) => {
                self.editor
                    .message_unlogged(said_of(&name, &why, &picked.title));
            }
            None if action.resolves => self.resolve_action(picked),
            None => match then {
                Some(then) => self.servers.command_next = Some(then),
                None => {
                    self.editor
                        .message_unlogged(said_of(&name, "no edit", &picked.title));
                }
            },
        }
        self.request_redraw();
    }

    /// Asks `picked`'s server to resolve it, where the person is now.
    fn resolve_action(&mut self, picked: Picked) {
        let (Some(index), Some(shown)) = (
            self.servers.find(&picked.server.0, &picked.server.1),
            self.editor.buffer(picked.buffer),
        ) else {
            return;
        };
        let at = AskAt {
            buffer: picked.buffer,
            revision: shown.revision(),
            offset: self.editor.window().selections.primary().cursor,
            agent: false,
        };
        let context = AskContext::Item {
            answer: picked.serial,
            index: picked.index,
        };
        self.servers.resolving = Some(picked);
        self.send_ask(index, (at, AskKind::ResolveAction, context), Who::Person);
    }

    /// Server `index` resolved the picked action for who asked, `taken`: its edit applied, if
    /// nothing moved since, then its command run.
    pub(super) fn code_action_resolved(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        resolved: ActionResolved,
    ) {
        let running = &self.servers.running[index];
        let picked = (self.servers.resolving.take()).filter(|picked| {
            picked.server.0 == running.root
                && picked.server.1 == running.entry.name
                && (picked.serial, picked.index) == (resolved.answer, resolved.index)
        });
        let (Some(picked), Some((asked, asker))) = (picked, taken) else {
            self.freer.free(resolved);
            return;
        };
        if !self.still_there(asked, &asker) {
            self.freer.free(resolved);
            return;
        }
        let name = picked.server.1.clone();
        let then = resolved.command.is_some().then(|| picked.clone());
        match (resolved.edit, then) {
            (Some(Ok(edit)), then) => {
                let (server, title) = (picked.server.clone(), picked.title.clone());
                self.apply_edit_of(&server, &title, edit, EditFrom::Action(then));
            }
            (Some(Err(why)), _) => {
                self.editor
                    .message_unlogged(said_of(&name, &why, &picked.title));
            }
            (None, Some(then)) => self.servers.command_next = Some(then),
            (None, None) => {
                self.editor
                    .message_unlogged(said_of(&name, "no edit", &picked.title));
            }
        }
        self.request_redraw();
    }

    /// At each sync, once the event's edits went to the server: asks the server of the action
    /// whose edit applied to run its command, a command it declared (Q5); the command's edits
    /// come as `workspace/applyEdit` while it waits.
    pub(super) fn run_command_next(&mut self) {
        let Some(then) = self.servers.command_next.take() else {
            return;
        };
        let (Some(index), Some(shown)) = (
            self.servers.find(&then.server.0, &then.server.1),
            self.editor.buffer(then.buffer),
        ) else {
            return;
        };
        let at = AskAt {
            buffer: then.buffer,
            revision: shown.revision(),
            offset: self.editor.window().selections.primary().cursor,
            agent: false,
        };
        let context = AskContext::Item {
            answer: then.serial,
            index: then.index,
        };
        self.send_ask(index, (at, AskKind::ExecuteCommand, context), Who::Person);
    }
}
