//! The UI's asks for code actions on the session thread (ADR 0138, step 13): an ask becomes a
//! request over its range with the diagnostics there as the server gave them, and an answer, or
//! a resolve, becomes actions for the UI with their edits placed (`edits`).
//!
//! Must not depend on: the process or the pipes; the run sends, and the reporter hands over.
//!
//! - The diagnostics given are those of the document's last report that overlap the range, if
//!   the report is of the text asked about, at most [`ACTION_DIAGNOSTICS_MAX`].
//! - The revisions of the texts the server has are kept as the request goes out: an edit of a
//!   document whose text changed while the server answered is refused (Q8).
//! - The actions are kept as the server gave them, but for the edits already read, for the
//!   picked one's resolve and a command's arguments, a resolved one as its resolve gave it; a
//!   newer answer replaces them.
//! - A command runs only if the server declared it (13b, Q5), with its arguments as given.

use oso_core::rope::Rope;
use serde_json::Value;

use crate::limits::ACTION_DIAGNOSTICS_MAX;
use crate::position;
use crate::protocol::{self, Reported};

use super::super::{ActionResolved, Answer, Answered, Ask, AskContext, CodeAction, CodeActions};
use super::Session;
use super::run::{LastItems, Run};

impl Session {
    /// `textDocument/codeAction`'s parameters for `ask`, over `start` to `end` of `text`, the
    /// document's at `uri`, or why it cannot be asked.
    pub(super) fn action_params(
        &self,
        run: &Run,
        ask: &Ask,
        (start, end): (usize, usize),
        (uri, text): (&str, &Rope),
    ) -> Result<Value, &'static str> {
        if !run.actions.acting.offers {
            return Err("it offers no code actions");
        }
        let start = position::from_offset(text, start, run.encoding);
        let end = position::from_offset(text, end, run.encoding).max(start);
        let report =
            (self.documents.path_of(ask.document)).and_then(|path| self.diagnosed.get(&path));
        // Only a report of the text asked about places its ranges in it.
        let version = self.documents.sent_version(ask.document);
        let diagnostics: Vec<&Reported> = (report.iter())
            .filter(|report| report.version.is_none() || report.version == version)
            .flat_map(|report| report.diagnostics.iter())
            .filter(|reported| reported.start <= end && start <= reported.end)
            .take(ACTION_DIAGNOSTICS_MAX)
            .collect();
        Ok(protocol::code_action_params(
            uri,
            (start, end),
            &diagnostics,
        ))
    }

    /// The code actions `result` offers for `ask`, request `id`, their edits placed.
    pub(super) fn offered_actions(
        &mut self,
        run: &mut Run,
        (id, ask): (i64, Ask),
        result: Value,
        took: u128,
    ) {
        let list = protocol::code_actions(&result);
        let base = run.actions.edit_bases.remove(&id);
        // Kept as the server gave them, but for the edits read, which may be large.
        let mut kept = protocol::take_actions(result, &list.at);
        for (value, offered) in kept.iter_mut().zip(&list.actions) {
            if offered.edit.is_some()
                && let Value::Object(fields) = value
            {
                fields.remove("edit");
            }
        }
        run.actions.last_actions = Some(LastItems {
            document: ask.document,
            serial: ask.serial,
            items: kept,
        });
        let resolves = run.actions.acting.resolves;
        let actions: Vec<CodeAction> = (list.actions.into_iter())
            .map(|offered| CodeAction {
                resolves: resolves && offered.resolvable,
                edit: (offered.edit).map(|edit| {
                    let edit = edit.map_err(str::to_owned)?;
                    self.placed(run.encoding, edit, base.as_ref())
                }),
                title: offered.title,
                kind: offered.kind,
                preferred: offered.preferred,
                disabled: offered.disabled,
                command: offered.command,
            })
            .collect();
        self.reporter.log(format!(
            "request {id} answered with {} code actions in {took} ms{}",
            actions.len(),
            if list.left > 0 {
                format!(", {} more left out", list.left)
            } else {
                String::new()
            }
        ));
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Ok(Answered::CodeActions(CodeActions { actions })),
        });
    }

    /// What `result`, a resolved code action, gives for `ask`, request `id`: its edit placed,
    /// and its command.
    pub(super) fn resolved_action(
        &mut self,
        run: &mut Run,
        (id, ask): (i64, Ask),
        mut result: Value,
        took: u128,
    ) {
        let AskContext::Item { answer, index } = ask.context else {
            self.fail(ask, "a resolve names no code action".to_owned());
            return;
        };
        let base = run.actions.edit_bases.remove(&id);
        let Some(resolved) = protocol::resolved_action(&result) else {
            self.fail(ask, "its resolve is no code action".to_owned());
            return;
        };
        // Kept as resolved, but for its edit, read already, for the command it may run (13b):
        // moved, not copied.
        if let Some(last) = run.actions.last_actions.as_mut()
            && (last.document, last.serial) == (ask.document, answer)
            && let Some(kept) = last.items.get_mut(index)
        {
            if let Value::Object(fields) = &mut result {
                fields.remove("edit");
            }
            *kept = result;
        }
        let edit = (resolved.edit).map(|edit| {
            let edit = edit.map_err(str::to_owned)?;
            self.placed(run.encoding, edit, base.as_ref())
        });
        self.reporter
            .log(format!("request {id} resolved a code action in {took} ms"));
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Ok(Answered::ActionResolved(ActionResolved {
                answer,
                index,
                edit,
                command: resolved.command,
            })),
        });
    }

    /// `workspace/executeCommand`'s parameters for the action at `index` of the answer `answer`
    /// about `document`, as the server gave it, if it runs a command the server declared.
    pub(super) fn command_params(
        &self,
        run: &Run,
        document: u64,
        (answer, index): (u64, usize),
    ) -> Result<Value, String> {
        let action = (run.actions.last_actions.as_ref())
            .filter(|last| (last.document, last.serial) == (document, answer))
            .and_then(|last| last.items.get(index))
            .ok_or("its code actions are gone")?;
        let (name, params) =
            protocol::execute_command_params(action).ok_or("it runs no command")?;
        // Only one the server declared it runs (Q5).
        if !run.actions.acting.commands.contains(&name) {
            return Err(format!(
                "the server did not declare {}",
                protocol::shown(&name)
            ));
        }
        Ok(params)
    }

    /// The title of the action whose command `ask` runs, as a list shows it.
    pub(super) fn command_title(&self, run: &Run, ask: &Ask) -> Option<String> {
        let AskContext::Item { answer, index } = ask.context else {
            return None;
        };
        let action = (run.actions.last_actions.as_ref())
            .filter(|last| (last.document, last.serial) == (ask.document, answer))
            .and_then(|last| last.items.get(index))?;
        Some(protocol::shown(action.get("title")?.as_str()?))
    }

    /// A command, request `id`, answered: its edits came while it waited.
    pub(super) fn executed(&mut self, id: i64, ask: Ask, took: u128) {
        self.reporter
            .log(format!("request {id} ran a command in {took} ms"));
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Ok(Answered::Executed),
        });
    }
}
