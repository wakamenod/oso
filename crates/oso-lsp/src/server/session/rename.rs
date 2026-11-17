//! A rename on the session thread (ADR 0138, step 14): a check of the place, asked only of a
//! server that checks one first, and the rename, whose answer is a workspace edit placed as a
//! code action's is (`edits`).
//!
//! Must not depend on: the process or the pipes; the run sends, and the reporter hands over.
//!
//! - A prepare's range is placed in the text it was asked about, within it, as every position
//!   is; one of a text gone is nothing to rename. Its placeholder is shown on one line
//!   (`protocol::prepared`).
//! - A rename's edit is placed against the revisions the server had as the request went out: a
//!   document whose text changed while the server answered refuses the whole edit (Q8).
//! - `*LSP*` gets the counts only: an edit's text may quote what the server read.

use serde_json::Value;

use crate::position;
use crate::protocol;

use super::super::{Answer, Answered, Ask, AskKind, Prepared};
use super::Session;
use super::run::Run;

impl Session {
    /// What `ask` gets at once, without a request: a rename of a server that renames nothing
    /// fails, and a check of the place by one that checks none first is unasked. `None` when it
    /// is to be asked.
    pub(super) fn answered_unasked(
        &mut self,
        run: &Run,
        ask: &Ask,
    ) -> Option<Result<(), &'static str>> {
        if !matches!(ask.kind, AskKind::PrepareRename | AskKind::Rename) {
            return None;
        }
        if !run.renaming.offers {
            return Some(Err("it renames nothing"));
        }
        if ask.kind == AskKind::PrepareRename && !run.renaming.prepares {
            self.answer_prepared(ask, Prepared::Unasked);
            return Some(Ok(()));
        }
        None
    }

    /// What `result`, the check of the place `ask` asked, request `id`, said: its range placed
    /// in the text asked about.
    pub(super) fn prepared_rename(
        &mut self,
        run: &Run,
        (id, ask): (i64, Ask),
        result: &Value,
        took: u128,
    ) {
        let prepared = match protocol::prepared(result) {
            None => Prepared::Nothing,
            Some(None) => Prepared::Unasked,
            Some(Some((start, end, placeholder))) => {
                match self.documents.as_sent(ask.document, ask.revision) {
                    Some((_, text)) => {
                        let range = position::to_range(text, start, end, run.encoding);
                        Prepared::At {
                            start: range.start,
                            end: range.end,
                            placeholder: placeholder.map(Into::into),
                        }
                    }
                    None => Prepared::Nothing,
                }
            }
        };
        self.reporter.log(format!(
            "request {id} checked a place to rename in {took} ms: {}",
            match prepared {
                Prepared::At { .. } => "a range",
                Prepared::Unasked => "the server's own way",
                Prepared::Nothing => "nothing there",
            }
        ));
        self.answer_prepared(&ask, prepared);
    }

    /// What `result`, the rename `ask` asked, request `id`, gave: its edit placed.
    pub(super) fn renamed(
        &mut self,
        run: &mut Run,
        (id, ask): (i64, Ask),
        result: &Value,
        took: u128,
    ) {
        let base = run.actions.edit_bases.remove(&id);
        if result.is_null() {
            self.fail(ask, "it renamed nothing there".to_owned());
            return;
        }
        let placed = protocol::workspace_edit(result)
            .map_err(str::to_owned)
            .and_then(|edit| self.placed(run.encoding, edit, base.as_ref()));
        let edit = match placed {
            Ok(edit) => edit,
            Err(why) => {
                self.fail(ask, why);
                return;
            }
        };
        let files = edit.open.len() + edit.unsent.len();
        let edits: usize = (edit.open.iter().map(|open| open.edits.len()))
            .chain(edit.unsent.iter().map(|unsent| unsent.edits.len()))
            .sum();
        self.reporter.log(format!(
            "request {id} renamed with {edits} edits in {files} files in {took} ms"
        ));
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Ok(Answered::Renamed(edit)),
        });
    }

    fn answer_prepared(&mut self, ask: &Ask, prepared: Prepared) {
        self.reporter.answer(Answer {
            serial: ask.serial,
            document: ask.document,
            kind: ask.kind,
            revision: ask.revision,
            outcome: Ok(Answered::Prepared(prepared)),
        });
    }
}
