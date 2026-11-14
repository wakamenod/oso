//! `oso mcp`'s session tools (ADR 0121, step 6): what each takes and says of itself. Each is
//! the channel's method of the same name; the sessions module holds the rules, and a test there
//! checks that these hints agree with them.
//!
//! Must not depend on: the socket or stdio; these only describe.

use serde_json::{Value, json};

use super::super::protocol::sessions::METHODS;
use super::tools::tool;

/// The channel's method of the session tool `name`, which is named the same, and the
/// arguments it takes; `None` if `name` is no session tool.
pub(super) fn method(name: &str) -> Option<(&'static str, &'static [&'static str])> {
    let arguments: &'static [&'static str] = match name {
        "session_list" => &["offset"],
        "session_state" => &["session"],
        "worktree_list" => &[],
        "worktree_remove" => &["path"],
        "start_worktree_session" => &["branch", "base", "brief", "name"],
        "session_start" => &["directory", "name", "brief"],
        _ => return None,
    };
    let method = METHODS.into_iter().find(|method| *method == name)?;
    Some((method, arguments))
}

/// The session tools that only read.
pub(super) fn reading() -> Vec<Value> {
    vec![
        tool(
            "session_list",
            "The agent sessions oso hosts, with each one's id, name, directory, state and age, the session that started it, and which one is you. Use it to follow the sessions you started. messaging_name is the name oso gave the session's claude; Claude Code may add a suffix when two sessions share a name, so check ListAgents before SendMessage.",
            json!({
                "offset": { "type": "integer", "minimum": 0, "description": "Where the list starts, when it holds more than one reply." },
            }),
            &[],
            true,
            false,
        ),
        tool(
            "session_state",
            "One agent session's fields, as session_list gives them.",
            json!({
                "session": { "type": "string", "description": "A session's id or name, from session_list." },
            }),
            &["session"],
            true,
            false,
        ),
        tool(
            "worktree_list",
            "The linked worktrees of your session's repository: each one's branch, its uncommitted and untracked files, whether its branch is merged and how that was found, the sessions in it, and whether worktree_remove would take it or why not. Needs a trusted project.",
            json!({}),
            &[],
            true,
            false,
        ),
    ]
}

/// The session tools that act.
pub(super) fn acting() -> Vec<Value> {
    vec![
        tool(
            "worktree_remove",
            "Remove a finished linked worktree of your session's repository: only when it has no uncommitted or untracked files, nothing unsaved in oso, no busy session, and its branch is merged. It never forces and never deletes the branch. The files git ignores there, such as build output, are deleted with it. A refusal names the check that failed: report it, and do not remove the worktree another way.",
            json!({
                "path": { "type": "string", "description": "The worktree's absolute path, from worktree_list." },
            }),
            &["path"],
            false,
            true,
        ),
        tool(
            "start_worktree_session",
            "Start a new agent session in a new worktree of your session's repository, on a branch, with a brief as its first prompt. The person is asked first; the call returns once they answer and the session starts. The brief gets the hand-off facts and a line saying to report back to you. Use this, never EnterWorktree or git worktree add.",
            json!({
                "branch": { "type": "string", "description": "The branch: an existing local one, or a new one." },
                "base": { "type": "string", "description": "Where a new branch starts; the commit your session is on if not given." },
                "brief": { "type": "string", "description": "The task for the new session, at most 64 KiB." },
                "name": { "type": "string", "description": "The session's name; the worktree's if not given." },
            }),
            &["branch", "brief"],
            false,
            false,
        ),
        tool(
            "session_start",
            "Start a new agent session in your session's repository, or one of its linked worktrees, with an optional brief as its first prompt. The person is asked first; the call returns once they answer and the session starts.",
            json!({
                "directory": { "type": "string", "description": "An absolute path: the repository's root or a linked worktree's; your session's if not given." },
                "name": { "type": "string" },
                "brief": { "type": "string", "description": "The task for the new session, at most 64 KiB." },
            }),
            &[],
            false,
            false,
        ),
    ]
}
