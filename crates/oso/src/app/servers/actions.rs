//! A server's code actions on the UI thread (ADR 0138, step 13, as `eglot-code-actions`): the
//! person's ask over the region or the cursor's place, the list its answer opens, and the
//! picked action's edit, resolved first if the server gives it only then, applied to the open
//! buffers it names.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.
//!
//! - Only the person asks (Q9): the commands are `Person`, so the agent channel never runs
//!   them, and the list is a float, which the agent never reads.
//! - The answer's actions are kept while the list is open and while the picked one resolves;
//!   a newer list replaces them.
//! - An edit applies whole or not at all, to buffers still at the text the server had (Q8). One
//!   that changes a file no buffer the server follows holds, and a command an action runs, wait
//!   for 13b; until then the echo area says so and nothing changes.
//! - An action's title is the server's text: the echo area shows it, and no log has it.

use oso_core::buffer::BufferId;
use oso_core::editor::{AskAt, BufferEdit, CodeActionItem};
use oso_lsp::server::{ActionResolved, AskContext, AskKind, CodeAction, CodeActions, ServerEdit};

use super::super::App;
use super::asks::{Asker, Who};

/// The actions of the list open now, or of the one whose picked action resolves: the server
/// that gave them, by its place in the running servers, the buffer asked about, and the ask's
/// serial, which a resolve names.
#[derive(Debug)]
pub(super) struct KeptActions {
    index: usize,
    buffer: BufferId,
    serial: u64,
    actions: Vec<CodeAction>,
}

impl App {
    /// The person's `C-c C-r`: asks the server following `at`'s buffer for the code actions
    /// from `at` to `end`, or says why not.
    pub(super) fn lsp_code_actions(&mut self, at: AskAt, end: usize) {
        let Some(index) = self.server_to_ask(at.buffer) else {
            return;
        };
        let context = AskContext::Range { end };
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
        let name = self.servers.running[index].entry.name.clone();
        if actions.actions.is_empty() {
            self.editor.message(format!("{name}: no code actions here"));
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
        if self
            .dispatcher
            .list_code_actions(&mut self.editor, &items)
            .is_some()
        {
            self.servers.actions = Some(KeptActions {
                index,
                buffer: asked,
                serial: asker.serial(),
                actions: actions.actions,
            });
        }
        self.request_redraw();
    }

    /// `lsp-code-action-apply`: the action the list took, applied, or resolved first.
    pub(super) fn apply_code_action(&mut self) {
        let (Some(hit), Some(kept)) = (
            self.editor.take_code_action_hit(),
            self.servers.actions.take(),
        ) else {
            return;
        };
        let Some(running) = self.servers.running.get(kept.index) else {
            return;
        };
        let name = running.entry.name.clone();
        let Some(action) = kept.actions.get(hit) else {
            return;
        };
        let title = action.title.clone();
        match &action.edit {
            Some(Ok(edit)) => self.apply_action_edit(kept.index, &title, edit),
            Some(Err(why)) => self.editor.message_unlogged(format!(
                "{name}: cannot apply \u{201c}{title}\u{201d}: {why}"
            )),
            None if action.resolves => {
                // Its edit comes with its resolve, asked where the person is now.
                let Some(buffer) = self.editor.buffer(kept.buffer) else {
                    return;
                };
                let at = AskAt {
                    buffer: kept.buffer,
                    revision: buffer.revision(),
                    offset: self.editor.window().selections.primary().cursor,
                    agent: false,
                };
                let context = AskContext::Item {
                    answer: kept.serial,
                    index: hit,
                };
                let index = kept.index;
                self.servers.actions = Some(kept);
                self.send_ask(index, (at, AskKind::ResolveAction, context), Who::Person);
            }
            None if action.command.is_some() => self.editor.message_unlogged(format!(
                "{name}: \u{201c}{title}\u{201d} runs a command, which oso does not run yet"
            )),
            None => self
                .editor
                .message_unlogged(format!("{name}: \u{201c}{title}\u{201d} has no edit")),
        }
        self.request_redraw();
    }

    /// Server `index` resolved the picked action for who asked, `taken`: its edit applied, if
    /// nothing moved since.
    pub(super) fn code_action_resolved(
        &mut self,
        index: usize,
        taken: Option<(BufferId, Asker)>,
        resolved: ActionResolved,
    ) {
        let kept = (self.servers.actions.take())
            .filter(|kept| (kept.index, kept.serial) == (index, resolved.answer));
        let (Some(kept), Some((asked, asker))) = (kept, taken) else {
            return;
        };
        if !self.still_there(asked, &asker) {
            return;
        }
        let Some(action) = kept.actions.get(resolved.index) else {
            return;
        };
        let title = action.title.clone();
        match &resolved.edit {
            Ok(edit) => self.apply_action_edit(index, &title, edit),
            Err(why) => {
                let name = self.servers.running[index].entry.name.clone();
                self.editor.message_unlogged(format!(
                    "{name}: cannot apply \u{201c}{title}\u{201d}: {why}"
                ));
            }
        }
        self.request_redraw();
    }

    /// Applies `edit`, server `index`'s edit of the action `title`, to the buffers it names,
    /// whole or not at all, and says what changed.
    fn apply_action_edit(&mut self, index: usize, title: &str, edit: &ServerEdit) {
        let running = &self.servers.running[index];
        let name = running.entry.name.clone();
        if !edit.unopened.is_empty() {
            let count = edit.unopened.len();
            let files = if count == 1 { "file" } else { "files" };
            self.editor.message_unlogged(format!(
                "{name}: \u{201c}{title}\u{201d} changes {count} {files} no buffer holds, which oso does not edit yet"
            ));
            return;
        }
        let mut parts = Vec::with_capacity(edit.open.len());
        for open in &edit.open {
            let Some(buffer) =
                (running.documents.keys().copied()).find(|id| id.get() == open.document)
            else {
                self.editor
                    .message(format!("{name}: a buffer the action edits is gone"));
                return;
            };
            parts.push(BufferEdit {
                buffer,
                revision: open.revision,
                edits: open.edits.clone(),
            });
        }
        let said = match self.editor.apply_server_edit(&parts) {
            Ok(applied) => {
                let edits = if applied.edits == 1 { "edit" } else { "edits" };
                let buffers = if applied.buffers == 1 {
                    "buffer"
                } else {
                    "buffers"
                };
                format!(
                    "{name}: \u{201c}{title}\u{201d}: {} {edits} in {} {buffers}",
                    applied.edits, applied.buffers
                )
            }
            Err(why) => format!("{name}: \u{201c}{title}\u{201d} not applied: {why}"),
        };
        self.editor.message_unlogged(said);
    }
}
