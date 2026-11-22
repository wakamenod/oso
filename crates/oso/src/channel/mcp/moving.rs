//! `oso mcp` stopping its own Claude Code session once that session moved into a space (ADR
//! 0151, step 2): after the reply to its `move_to_space` has gone to the agent, the bridge
//! holds the channel's `move_wait`, and the app answers once the session's turn has ended.
//!
//! Must not depend on: the registry or the app; oso watches the session, and the bridge only
//! signals its own parent.
//!
//! - Only after a reply of the bridge's own call that says the person agreed, on the
//!   connection it was made on, so nothing new is asked.
//! - `SIGTERM` goes only to the process the app names, and only while it is this process's
//!   parent (`oso_platform::process::terminate_parent`). Anything else, including a refusal or
//!   a broken connection, stops nothing, and the person's `/exit` is what is left.
//! - The agent never waits on it: a request it sends meanwhile cuts the wait, as a cancel
//!   does, and is answered; the session is then not stopped.
//! - Windows has no such stop: the bridge holds nothing there.

use serde_json::{Value, json};

use super::{Bridge, say};

/// Whether the reply to the tool `name` says the person agreed to a move of this session.
pub(super) fn agreed(name: &str, result: &Value) -> bool {
    name == "move_to_space" && result["moving"] == Value::Bool(true)
}

/// The process the reply to `move_wait` names to stop, if it names one.
pub(super) fn to_stop(reply: &Value) -> Option<u32> {
    reply["stop"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
}

impl Bridge {
    /// After the response to the request `id`, whose move the person agreed to, has been
    /// written: waits for the session's turn to end, then stops its own parent.
    pub(super) fn stop_parent_once_idle(&mut self, id: &Value) {
        // The wait goes on the connection the move was asked on, so nothing new is asked.
        if !cfg!(unix) || self.channel.link.is_none() {
            return;
        }
        // Any request of the agent's ends the wait: the turn has not ended then.
        self.interrupt.lock().yields = true;
        let waited = self.call(id, super::Which::Channel, "move_wait", json!({}));
        self.interrupt.lock().yields = false;
        let reply = match waited {
            Ok(reply) => reply,
            Err(failed) => {
                return say(&format!(
                    "this session is not stopped, so type /exit: {}",
                    failed.message()
                ));
            }
        };
        let Some(pid) = to_stop(&reply) else {
            return say("this session is not stopped, so type /exit: oso named no process");
        };
        match oso_platform::process::terminate_parent(pid) {
            Ok(()) => say("asked this session to end, as it moves into oso"),
            Err(e) => say(&format!("this session is not stopped, so type /exit: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{agreed, to_stop};

    #[test]
    fn only_a_move_the_person_agreed_to_waits_and_only_a_named_process_stops() {
        assert!(agreed("move_to_space", &json!({ "moving": true })));
        assert!(!agreed("move_to_space", &json!({ "moving": "true" })));
        assert!(!agreed("move_to_space", &json!({})));
        assert!(!agreed("session_start", &json!({ "moving": true })));
        assert_eq!(to_stop(&json!({ "stop": 812 })), Some(812));
        assert_eq!(to_stop(&json!({ "stop": -1 })), None);
        assert_eq!(to_stop(&json!({ "stop": u64::MAX })), None);
        assert_eq!(to_stop(&json!({ "stop": "812" })), None);
        assert_eq!(to_stop(&json!({})), None);
    }
}
