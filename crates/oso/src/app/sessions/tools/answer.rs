//! `session_answer` in the app (ADR 0186): a space's orchestrator's one answer to the request
//! a native worker of its space shows the person. The rules are `sessions::tools::answer`'s and
//! `sessions::native::stand_in`'s; this gathers what they judge and carries the answer out.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk or
//! starts a process; the answer goes to the worker's bridge as the person's would.
//!
//! - The caller is judged as it is now: the current orchestrator of the worker's space, never
//!   the worker itself. The request is the one shown now, by oso's id and its tool, judged
//!   under the rules in force now, so that a change of the person's file takes the next call.
//! - The person sees each answer: on the call's row, in the echo area and in the agent log.

use serde_json::json;

use super::super::super::App;
use super::Caller;
use crate::channel::Reply;
use crate::git::printable;
use crate::sessions::native::asks::RequestId;
use crate::sessions::tools::answer::{self as rules, Answering, ONLY_ORCHESTRATOR, Shown, Verdict};

/// The most characters of a session's name an echo shows.
const NAME: usize = 64;

impl App {
    /// `session_answer` from `caller`: `answer`, `allow` or `deny`, to the request `request`,
    /// of `tool`, that `session` shows the person, with a deny's `reason`.
    pub(super) fn session_answer(
        &mut self,
        caller: Option<&Caller>,
        session: &str,
        request: &str,
        tool: &str,
        answer: &str,
        reason: Option<&str>,
    ) -> Reply {
        // A worker learns nothing of which sessions or requests there are.
        let caller = caller
            .filter(|caller| caller.orchestrator())
            .ok_or(ONLY_ORCHESTRATOR)?;
        let space = caller.space.as_ref().ok_or(ONLY_ORCHESTRATOR)?;
        let verdict = Verdict::parse(answer).ok_or("answer is \"allow\" or \"deny\"")?;
        let found = self.find_session(Some(caller), session)?;
        let (key, id, worker) = (found.key, found.id.clone(), found.name.clone());
        let (native, directory) = (found.native, found.directory.clone());
        let of_its_space = self.session_in_space(found, &space.id);
        // As it is now: an earlier orchestrator, or a session in the space's own directory, is
        // no worker.
        let a_worker =
            (self.caller_of(found).space).is_some_and(|at| at.role == super::space::Role::Worker);
        let named = RequestId::parse(request);
        let shown = self.native_request_shown(key, &directory);
        let answered = named.and_then(|named| self.native_request_answered(key, named));
        rules::judge(&Answering {
            orchestrator: true,
            of_its_space,
            worker: a_worker,
            own: key == caller.key,
            native,
            request: named,
            tool,
            verdict,
            reason,
            shown: shown.as_ref().map(|shown| Shown {
                id: shown.id,
                tool: &shown.tool,
                card: shown.card,
                verdict: shown.verdict,
            }),
            answered,
            person_answering: self.native_person_answering(key),
        })?;
        let (Some(named), Some(shown)) = (named, shown) else {
            return Err("that request no longer waits".into());
        };
        let table = crate::sessions::tools::tool("session_answer").ok_or("no such session tool")?;
        self.admit_tool(table, caller, id.as_str())?;
        let name = printable(&caller.name, NAME);
        if !self.stand_in_answers(key, named, verdict, (&name, reason)) {
            return Err("that request no longer waits".into());
        }
        let said = rules::said(
            &name,
            verdict,
            &printable(tool, NAME),
            &printable(&worker, NAME),
        );
        self.agent_log(&format!("session_answer: {said}"));
        self.editor.message(said);
        Ok(json!({
            "session": id.as_str(),
            "request": named.to_string(),
            "tool": tool,
            "answer": verdict.said(),
            "summary": shown.summary,
        }))
    }
}
