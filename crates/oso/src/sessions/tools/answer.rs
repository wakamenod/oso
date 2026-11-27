//! The rules of `session_answer` (ADR 0186): which request a space's orchestrator may allow
//! or deny for a native worker of its space, and what the worker's bridge and the person are
//! told of it.
//!
//! Must not depend on: the app or threads; the rules are plain data and functions over what
//! the app found.
//!
//! - Only the current orchestrator of the worker's space answers, a worker's request only, never
//!   its own nor an earlier orchestrator's, and
//!   only the request shown to the person now, named by oso's id and its tool.
//! - A request the person answered, one answered already, and one whose session's person is
//!   answering it now (their `C-u y` question or a note open) are refused.
//! - Questions and plans are the person's: neither allowed nor denied. Any other request the
//!   rules keep for the person may be denied, never allowed.

use super::quoted;
use crate::sessions::native::asks::{Answerer, RequestId};
use crate::sessions::native::stand_in::PersonOnly;

/// The most characters a deny's reason holds.
pub const MAX_REASON: usize = crate::channel::protocol::sessions::MAX_REASON_CHARS;

/// What a caller that is no current orchestrator of a space is told.
pub const ONLY_ORCHESTRATOR: &str = "only your space's orchestrator answers requests";

/// What an answer is told while the person's `C-u y` question or a note is open for its request.
pub const PERSON_ANSWERING: &str = "the person is answering it: wait";

/// What the orchestrator answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny,
}

impl Verdict {
    /// The answer `text` names: `allow` or `deny`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    /// As the echo area and the reply say it.
    pub fn said(self) -> &'static str {
        match self {
            Self::Allow => "allowed",
            Self::Deny => "denied",
        }
    }
}

/// The request a session shows the person now, as the rules judge it.
#[derive(Clone, Copy, Debug)]
pub struct Shown<'a> {
    pub id: RequestId,
    pub tool: &'a str,
    /// It is a question or a plan, on the session's card.
    pub card: bool,
    /// Whether the orchestrator may allow it, by `stand_in::eligible`, or why only the person may.
    pub verdict: Result<(), PersonOnly>,
}

/// What the app found of a `session_answer` call, and the call itself.
#[derive(Clone, Copy, Debug)]
pub struct Answering<'a> {
    /// The caller is its space's current orchestrator.
    pub orchestrator: bool,
    /// The session named is of the caller's space.
    pub of_its_space: bool,
    /// The session named is a worker there: no orchestrator, current or earlier, and no
    /// session in the space's own directory.
    pub worker: bool,
    /// The session named is the caller.
    pub own: bool,
    /// The session named runs natively, with its requests asked in oso.
    pub native: bool,
    /// The request the call names, if its text is one of oso's ids.
    pub request: Option<RequestId>,
    /// The tool the call says the request is of.
    pub tool: &'a str,
    pub verdict: Verdict,
    pub reason: Option<&'a str>,
    /// The request the session shows the person now, if one.
    pub shown: Option<Shown<'a>>,
    /// Who answered the request named lately, if anyone did.
    pub answered: Option<Answerer>,
    /// The person's `C-u y` question, or a note, is open for the session's request.
    pub person_answering: bool,
}

/// Whether the orchestrator's answer goes to the request shown, or why it is refused.
///
/// # Errors
///
/// Why it is refused, for the orchestrator.
pub fn judge(call: &Answering<'_>) -> Result<(), String> {
    if !call.orchestrator {
        return Err(ONLY_ORCHESTRATOR.into());
    }
    if !call.of_its_space {
        return Err(
            "that session is not of your space: you answer your own space's workers only".into(),
        );
    }
    if call.own {
        return Err("that is your own request: only the person answers it".into());
    }
    if !call.worker {
        return Err(
            "that session is not a worker of your space: only workers' requests are answered"
                .into(),
        );
    }
    if !call.native {
        return Err(
            "that session is not native: only a native session's requests are answered, and a terminal's are the person's"
                .into(),
        );
    }
    let Some(request) = call.request else {
        return Err("request is oso's id of it, such as r17: give request.id as listed".into());
    };
    match call.answered {
        Some(Answerer::Person) => return Err("the person answered it first".into()),
        Some(Answerer::StandIn) => return Err("that request was answered already".into()),
        None => {}
    }
    let shown = (call.shown)
        .filter(|shown| shown.id == request)
        .ok_or("that request no longer waits: call session_state for the one shown now")?;
    if shown.tool != call.tool {
        return Err(format!(
            "that request is of {}, not the tool you named: call session_state again",
            quoted(shown.tool)
        ));
    }
    if call.person_answering {
        return Err(PERSON_ANSWERING.into());
    }
    if shown.card {
        return Err(PersonOnly::Card.says().into());
    }
    match (call.verdict, call.reason) {
        (Verdict::Allow, Some(_)) => {
            return Err("a reason is a deny's alone: allow takes none".into());
        }
        (Verdict::Deny, Some(reason)) if !plain_reason(reason) => {
            return Err(
                "a deny's reason is 1 to 200 plain characters: no line break, control or invisible character"
                    .into(),
            );
        }
        _ => {}
    }
    match call.verdict {
        Verdict::Allow => shown.verdict.map_err(|why| why.says().into()),
        Verdict::Deny => Ok(()),
    }
}

/// Whether `reason` is one a deny may carry: 1 to [`MAX_REASON`] characters, none a control or
/// one that would not show.
pub fn plain_reason(reason: &str) -> bool {
    let shows = |c: char| !c.is_control() && !oso_core::display_map::cell::is_invisible(c);
    !reason.is_empty() && reason.chars().count() <= MAX_REASON && reason.chars().all(shows)
}

/// What the worker's bridge tells its CLI of a deny by orchestrator `name`, with `reason`.
pub fn denial(name: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) => {
            format!("The space's orchestrator {name} denied this call in oso, saying: {reason}")
        }
        None => format!("The space's orchestrator {name} denied this call in oso."),
    }
}

/// What the denied call's row says: "denied: by NAME".
pub fn denied_row(name: &str) -> String {
    format!("by {name}")
}

/// What the echo area and the agent log say: "NAME allowed TOOL in WORKER".
pub fn said(name: &str, verdict: Verdict, tool: &str, worker: &str) -> String {
    format!("{name} {} {tool} in {worker}", verdict.said())
}

#[cfg(test)]
mod tests;
