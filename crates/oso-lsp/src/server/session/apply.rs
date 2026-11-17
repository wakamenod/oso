//! A server's `workspace/applyEdit` on the session thread (ADR 0138, step 13b, Q5): while a
//! command oso asked it to run waits, and no other edit waits for the UI, the edit is placed and
//! handed to the UI, whose answer goes back; at any other time it is refused at once.
//!
//! Must not depend on: the process or the pipes; the run sends, and the reporter hands over.
//!
//! - Its edit is placed as a code action's is (`edits`), against the texts the server has as it
//!   asks: a buffer changed before the UI applies it is refused there.
//! - The answer the UI gives is `applied: true`, or `applied: false` with why; a token the
//!   session no longer waits for is let go.

use serde_json::{Value, json};

use crate::protocol::{self, RequestId};

use super::super::ApplyEdit;
use super::Session;
use super::run::Run;

/// Why a `workspace/applyEdit` that came at any other time is refused.
const NOT_NOW: &str = "oso applies a server's edit only while a command it asked for waits";

impl Session {
    /// The server asks, as request `id`, to apply the edit of `params`.
    pub(super) fn apply_edit_asked(
        &mut self,
        run: &mut Run,
        id: RequestId,
        params: &Value,
    ) -> Result<(), String> {
        if run.actions.executing.is_empty() || run.actions.applying.is_some() {
            self.reporter
                .log("asked workspace/applyEdit, answered not applied".to_owned());
            let refused = json!({"applied": false, "failureReason": NOT_NOW});
            return run.send(&protocol::answer(&id, Ok(refused)));
        }
        let (label, edit) = protocol::applied_edit(params);
        let base = self.documents.sent_revisions();
        let edit = match edit {
            Some(Ok(edit)) => self.placed(run.encoding, edit, Some(&base)),
            Some(Err(why)) => Err(why.to_owned()),
            None => Err("it gives no edit".to_owned()),
        };
        run.actions.apply_serial += 1;
        let token = run.actions.apply_serial;
        run.actions.applying = Some((token, id));
        self.reporter
            .log("asked workspace/applyEdit, waiting for the person".to_owned());
        self.reporter.apply_edit(ApplyEdit { token, label, edit });
        Ok(())
    }

    /// The UI's answer to the `workspace/applyEdit` of `token`.
    pub(super) fn answer_apply_edit(
        &mut self,
        run: &mut Run,
        token: u64,
        outcome: Result<(), String>,
    ) -> Result<(), String> {
        let id = match run.actions.applying.take() {
            Some((waiting, id)) if waiting == token => id,
            other => {
                run.actions.applying = other;
                return Ok(());
            }
        };
        let answer = match &outcome {
            Ok(()) => json!({"applied": true}),
            Err(why) => json!({"applied": false, "failureReason": why}),
        };
        self.reporter.log(format!(
            "answered workspace/applyEdit, {}",
            if outcome.is_ok() {
                "applied"
            } else {
                "not applied"
            }
        ));
        run.send(&protocol::answer(&id, Ok(answer)))
    }
}
