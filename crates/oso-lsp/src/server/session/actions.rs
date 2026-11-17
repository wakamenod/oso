//! The UI's asks for code actions on the session thread (ADR 0138, step 13): an ask becomes a
//! request over its range with the diagnostics there as the server gave them, and an answer, or
//! a resolve, becomes actions for the UI with their edits placed in the texts the server has.
//!
//! Must not depend on: the process or the pipes; the run sends, and the reporter hands over.
//!
//! - The diagnostics given are those of the document's last report that overlap the range, if
//!   the report is of the text asked about, at most
//!   [`ACTION_DIAGNOSTICS_MAX`](crate::limits::ACTION_DIAGNOSTICS_MAX).
//! - An edit of a document open with the server is placed in the text it has; one that names a
//!   version must name that text's, or it is refused (Q8). A file oso never opens, under an
//!   automount or with a `..`, refuses the whole edit.
//! - The actions are kept as the server gave them while it resolves, for the picked one's
//!   resolve; a newer answer replaces them.

use std::path::Path;

use oso_core::rope::Rope;
use serde_json::Value;

use crate::limits::ACTION_DIAGNOSTICS_MAX;
use crate::position::{self, Encoding};
use crate::protocol::{self, Reported, WireEdit};
use crate::uri;

use super::super::{
    ActionResolved, Answer, Answered, Ask, AskContext, CodeAction, CodeActions, OpenEdit,
    ServerEdit, UnopenedEdit,
};
use super::Session;
use super::asks::automounted;
use super::run::{LastItems, Run};

impl Session {
    /// `textDocument/codeAction`'s parameters for `ask`, over its offset to `end` of `text`, the
    /// document's at `uri`, or why it cannot be asked.
    pub(super) fn action_params(
        &self,
        run: &Run,
        ask: &Ask,
        end: usize,
        (uri, text): (&str, &Rope),
    ) -> Result<Value, &'static str> {
        if !run.acting.offers {
            return Err("it offers no code actions");
        }
        let start = position::from_offset(text, ask.offset, run.encoding);
        let end = position::from_offset(text, end.max(ask.offset), run.encoding);
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
        // Kept for their resolve, taken out of the answer; the last ones go first.
        run.last_actions = None;
        if run.acting.resolves {
            run.last_actions = Some(LastItems {
                document: ask.document,
                serial: ask.serial,
                items: protocol::take_actions(result, &list.at),
            });
        }
        let resolves = run.acting.resolves;
        let actions: Vec<CodeAction> = (list.actions.into_iter())
            .map(|offered| CodeAction {
                resolves: resolves && offered.resolvable,
                edit: (offered.edit).map(|edit| {
                    edit.map_err(str::to_owned)
                        .and_then(|edit| self.placed(run.encoding, edit))
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

    /// The edit `result`, a resolved code action, gives for `ask`, request `id`, placed.
    pub(super) fn resolved_action(
        &mut self,
        run: &Run,
        (id, ask): (i64, Ask),
        result: &Value,
        took: u128,
    ) {
        let AskContext::Item { answer, index } = ask.context else {
            self.fail(ask, "a resolve names no code action".to_owned());
            return;
        };
        let edit = match protocol::resolved_action(result) {
            Some(Ok(edit)) => self.placed(run.encoding, edit),
            Some(Err(why)) => Err(why.to_owned()),
            None => Err("its resolve gave no edit".to_owned()),
        };
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
            })),
        });
    }

    /// `edit` placed: each file's edits in the text the server has of the document open at its
    /// path, merged by document, and the others' kept as the server counts them; or why it
    /// cannot be applied.
    fn placed(&self, encoding: Encoding, edit: WireEdit) -> Result<ServerEdit, String> {
        let sent = self.documents.sent_by_path();
        let mut placed = ServerEdit::default();
        for file in edit.files {
            let path = (uri::to_path(&file.uri))
                .filter(|path| !automounted(path))
                .ok_or("it names a file oso does not edit")?;
            let Some(&(document, revision, text)) = sent.get(Path::new(&path)) else {
                match placed.unopened.iter_mut().find(|kept| kept.path == path) {
                    Some(kept) => kept.edits.extend(file.edits),
                    None => placed.unopened.push(UnopenedEdit {
                        path,
                        edits: file.edits,
                        encoding,
                    }),
                }
                continue;
            };
            if file.version.is_some() && file.version != self.documents.sent_version(document) {
                return Err("it was made for an older text of a buffer".to_owned());
            }
            let edits = (file.edits.into_iter())
                .map(|(start, end, new)| (position::to_range(text, start, end, encoding), new));
            match placed
                .open
                .iter_mut()
                .find(|kept| kept.document == document)
            {
                Some(kept) => kept.edits.extend(edits),
                None => placed.open.push(OpenEdit {
                    document,
                    revision,
                    edits: edits.collect(),
                }),
            }
        }
        Ok(placed)
    }
}
