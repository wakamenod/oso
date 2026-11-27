//! A native worker's request as its space's orchestrator sees and answers it (ADR 0186): what
//! it is listed, what `session_answer` judges, and its answer carried out as the person's card
//! carries one out. The rules are `sessions::native::stand_in`'s and `sessions::tools::answer`'s.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - The request's input stays here: the session tools' files read only what these give them.

use std::time::Instant;

use serde_json::Value;

use super::card;
use crate::app::App;
use crate::sessions::native::asks::{Answerer, RequestId};
use crate::sessions::native::stand_in;
use crate::sessions::native::wire::tools::decode;
use crate::sessions::tools::answer::{self, Verdict};
use crate::sessions::worker::SessionKey;

/// A native session's request shown to the person, as `session_answer` judges it (ADR 0186).
#[derive(Debug)]
pub(in crate::app) struct RequestShown {
    pub(in crate::app) id: RequestId,
    pub(in crate::app) tool: String,
    /// A question or a plan, on the card.
    pub(in crate::app) card: bool,
    /// Whether the orchestrator may allow it, or why only the person may.
    pub(in crate::app) verdict: Result<(), stand_in::PersonOnly>,
    /// What it works on, as the call's row has it; none where the listing withholds its input.
    pub(in crate::app) summary: Option<String>,
}

impl App {
    /// Native session `key`'s request shown to the person, if one is, as its space's
    /// orchestrator is listed it at `now`, of the session in `directory` (ADR 0186). Only the
    /// session tools' listing for that orchestrator calls this.
    pub(in crate::app) fn native_request_listed(
        &self,
        key: SessionKey,
        directory: &std::path::Path,
        now: Instant,
    ) -> Option<Value> {
        let ask = self.hosting.natives.permissions.asks.shown(key)?;
        let shown_s = (ask.shown).map_or(0, |shown| now.saturating_duration_since(shown).as_secs());
        let card = ask.question().is_some();
        let rules = self.configuration.stand_in();
        Some(stand_in::listing(
            ask.id, &ask.call, card, shown_s, directory, rules,
        ))
    }

    /// Native session `key`'s request shown to the person, if one is, as `session_answer` judges
    /// it for the session in `directory` under the rules in force now (ADR 0186).
    pub(in crate::app) fn native_request_shown(
        &self,
        key: SessionKey,
        directory: &std::path::Path,
    ) -> Option<RequestShown> {
        let ask = self.hosting.natives.permissions.asks.shown(key)?;
        let card = ask.question().is_some();
        let rules = self.configuration.stand_in();
        let tool = ask.call.tool_name.as_str();
        let verdict = stand_in::eligible(tool, ask.call.input.as_ref(), card, directory, rules);
        // What the listing withholds, the reply does too, and more.
        let summary =
            stand_in::shows_summary(verdict).then(|| summary(tool, ask.call.input.as_ref()));
        Some(RequestShown {
            id: ask.id,
            tool: tool.to_owned(),
            card,
            verdict,
            summary,
        })
    }

    /// Who answered native session `key`'s request `id` lately, if anyone did (ADR 0186).
    pub(in crate::app) fn native_request_answered(
        &self,
        key: SessionKey,
        id: RequestId,
    ) -> Option<Answerer> {
        self.hosting.natives.permissions.asks.answered_by(key, id)
    }

    /// Whether the person is answering native session `key`'s request now: their `C-u y`
    /// question about it, or a note for it, is open (ADR 0186).
    pub(in crate::app) fn native_person_answering(&self, key: SessionKey) -> bool {
        let permissions = &self.hosting.natives.permissions;
        (permissions.writing.as_ref()).is_some_and(|(writing, _)| writing.session() == key)
            || self.unread_question_for(key)
    }

    /// The space's orchestrator `name` answers native session `key`'s request `id` as `verdict`
    /// says, with a deny's `reason`: an allow is the one allow, with no updated permissions;
    /// the call's row says who. Whether that request was the one shown.
    pub(in crate::app) fn stand_in_answers(
        &mut self,
        key: SessionKey,
        id: RequestId,
        verdict: Verdict,
        (name, reason): (&str, Option<&str>),
    ) -> bool {
        let asks = &self.hosting.natives.permissions.asks;
        if asks
            .shown(key)
            .is_none_or(|ask| ask.id != id || ask.question().is_some())
        {
            return false;
        }
        let answer = match verdict {
            Verdict::Allow => card::Answer::Allow(card::By::Stand(name.to_owned())),
            Verdict::Deny => card::Answer::Deny(
                answer::denial(name, reason),
                answer::denied_row(name),
                card::By::Stand(name.to_owned()),
            ),
        };
        self.answer_card(key, answer);
        true
    }
}

/// What a call of `tool` with `value` works on, as its row says it, on one line and cut: its
/// path and its command or pattern.
fn summary(tool: &str, value: Option<&Value>) -> String {
    let read = decode(tool, value);
    let parts: Vec<String> = [read.path, read.text]
        .into_iter()
        .flatten()
        .map(|part| crate::git::printable(&part.replace(['\n', '\r'], " "), 200))
        .collect();
    parts.join(" · ")
}
