//! A server's code actions on the UI thread (ADR 0138, step 13, as `eglot-code-actions`): the
//! person's ask over the region or the cursor's place, the list its answer opens, and the
//! picked action's edit, resolved first if the server gives it only then, applied to the open
//! buffers it names.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.
//!
//! - Only the person asks (Q9): the commands are `Person`, so the agent channel never runs
//!   them, and the list is a float, which the agent never reads.
//! - The answer's actions are kept, with the server named by its root and entry, until one is
//!   picked or a newer list opens; the picked one's resolve keeps only what it needs. What is let
//!   go is freed off the UI thread.
//! - An edit applies whole or not at all, to buffers still at the text the server had (Q8), and
//!   files whose text the server does not have open first (`applying`, 13b). An action's command
//!   runs after its edit applied, only one the server declared (Q5).
//! - An action's title is the server's text, with its bidi and invisible characters written out:
//!   the echo area shows it after what changed, and no log has it.

use std::path::PathBuf;

use oso_core::buffer::BufferId;
use oso_core::editor::{AskAt, CodeActionItem};
use oso_lsp::server::{ActionResolved, AskContext, AskKind, CodeAction, CodeActions};

use super::super::App;
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

/// A command to run once its action's edit applied: its server, the buffer asked about, its
/// action by its answer's serial and its place, and the action's title (13b).
#[derive(Debug)]
pub(super) struct Then {
    server: (PathBuf, String),
    buffer: BufferId,
    serial: u64,
    index: usize,
    title: String,
}

/// The picked action while it resolves: its server, its answer's serial, its place and title.
#[derive(Debug)]
pub(super) struct Resolving {
    server: (PathBuf, String),
    serial: u64,
    index: usize,
    title: String,
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
        let (server, buffer, revision, serial) =
            (kept.server.clone(), kept.buffer, kept.revision, kept.serial);
        self.freer.free(kept);
        let name = server.1.clone();
        let title = action.title;
        let then = (action.command.is_some()).then(|| Then {
            server: server.clone(),
            buffer,
            serial,
            index: hit,
            title: title.clone(),
        });
        if self
            .editor
            .buffer(buffer)
            .map(oso_core::buffer::Buffer::revision)
            != Some(revision)
        {
            self.editor.message_unlogged(format!(
                "{name}: the buffer changed since the list opened: \u{201c}{title}\u{201d}"
            ));
        } else {
            // An edit goes first, then the command it runs, as the protocol orders them.
            match action.edit {
                Some(Ok(edit)) => self.apply_edit_of(&server, &title, edit, (then, None)),
                Some(Err(why)) => self
                    .editor
                    .message_unlogged(format!("{name}: {why}: \u{201c}{title}\u{201d}")),
                None if action.resolves => {
                    self.resolve_action(server, (buffer, serial, hit), title);
                }
                None => match then {
                    Some(then) => self.run_then(then),
                    None => self
                        .editor
                        .message_unlogged(format!("{name}: no edit: \u{201c}{title}\u{201d}")),
                },
            }
        }
        self.request_redraw();
    }

    /// Asks `server` to resolve the action at `hit` of the answer `serial` about `buffer`, where
    /// the person is now.
    fn resolve_action(
        &mut self,
        server: (PathBuf, String),
        (buffer, serial, hit): (BufferId, u64, usize),
        title: String,
    ) {
        let (Some(index), Some(shown)) = (
            self.servers.find(&server.0, &server.1),
            self.editor.buffer(buffer),
        ) else {
            return;
        };
        let at = AskAt {
            buffer,
            revision: shown.revision(),
            offset: self.editor.window().selections.primary().cursor,
            agent: false,
        };
        let context = AskContext::Item {
            answer: serial,
            index: hit,
        };
        self.servers.resolving = Some(Resolving {
            server,
            serial,
            index: hit,
            title,
        });
        self.send_ask(index, (at, AskKind::ResolveAction, context), Who::Person);
    }

    /// Server `index` resolved the picked action for who asked, `taken`: its edit applied, if
    /// nothing moved since.
    pub(super) fn code_action_resolved(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        resolved: ActionResolved,
    ) {
        let running = &self.servers.running[index];
        let resolving = (self.servers.resolving.take()).filter(|resolving| {
            resolving.server.0 == running.root
                && resolving.server.1 == running.entry.name
                && (resolving.serial, resolving.index) == (resolved.answer, resolved.index)
        });
        let (Some(resolving), Some((asked, asker))) = (resolving, taken) else {
            self.freer.free(resolved);
            return;
        };
        if !self.still_there(asked, &asker) {
            self.freer.free(resolved);
            return;
        }
        let (name, title) = (resolving.server.1.clone(), resolving.title.clone());
        let then = (resolved.command.is_some()).then(|| Then {
            server: resolving.server.clone(),
            buffer: asked,
            serial: resolving.serial,
            index: resolving.index,
            title: title.clone(),
        });
        match (resolved.edit, then) {
            (Some(Ok(edit)), then) => {
                self.apply_edit_of(&resolving.server, &title, edit, (then, None));
            }
            (Some(Err(why)), _) => self
                .editor
                .message_unlogged(format!("{name}: {why}: \u{201c}{title}\u{201d}")),
            (None, Some(then)) => self.run_then(then),
            (None, None) => self
                .editor
                .message_unlogged(format!("{name}: no edit: \u{201c}{title}\u{201d}")),
        }
        self.request_redraw();
    }

    /// Asks `then`'s server to run the command of its action, a command it declared (Q5); its
    /// edits come as `workspace/applyEdit` while it waits.
    pub(super) fn run_then(&mut self, then: Then) {
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
        self.servers.running_command = Some(then.title);
        self.send_ask(index, (at, AskKind::ExecuteCommand, context), Who::Person);
    }
}
