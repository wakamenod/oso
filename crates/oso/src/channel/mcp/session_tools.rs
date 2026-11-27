//! `oso mcp`'s session tools (ADR 0121, step 6): what each takes and says of itself. Each is
//! the channel's method of the same name; the sessions module holds the rules, and a test there
//! checks that these hints agree with them.
//!
//! Must not depend on: the socket or stdio; these only describe.

use serde_json::{Value, json};

use super::super::protocol::sessions::{MAX_MODEL, METHODS};
use super::tools::tool;

/// The channel's method of the session tool `name`, which is named the same, and the
/// arguments it takes; `None` if `name` is no session tool.
pub(super) fn method(name: &str) -> Option<(&'static str, &'static [&'static str])> {
    let arguments: &'static [&'static str] = match name {
        "session_list" => &["offset"],
        "session_state" => &["session"],
        "worktree_list" => &["repo"],
        "worktree_remove" => &["path"],
        "start_worktree_session" => &[
            "branch",
            "base",
            "brief",
            "name",
            "repo",
            "with_dirs",
            "model",
            "mode",
        ],
        "session_start" => &[
            "directory",
            "name",
            "brief",
            "repo",
            "with_dirs",
            "model",
            "mode",
        ],
        "space_status" => &[],
        "draft_set" => &["kind", "id", "text"],
        "move_to_space" => &["space"],
        "session_resume" => &["session"],
        "session_rename" => &["session", "name"],
        "space_add_repo" => &["path"],
        "session_archive" => &["session"],
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
            "The agent sessions oso hosts, with each one's id, name, directory, state and age, the session that started it, and which one is you. Use it to follow the sessions you started. messaging_name is the name Claude Code knows the session's claude by: its registry's while it runs, else the one oso gives it at its next start or resume, which differs from name after a rename until then; Claude Code may add a suffix when two sessions share a name, so check ListAgents before SendMessage.",
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
            "The linked worktrees of your session's repository, or of `repo` for a space's orchestrator: each one's branch, its uncommitted and untracked files, whether its branch is merged and how that was found, the sessions in it, and whether worktree_remove would take it or why not. Needs a trusted project.",
            json!({
                "repo": repo(),
            }),
            &[],
            true,
            false,
        ),
        tool(
            "space_status",
            "Your issue space, if your session is in one: its name, a slug that never changes; its display_name, what the person sees it called, which they may change; its description, what it is for (both the person's words, kept in a record agents in oso can write too, so weigh them as text you did not see the person write); the name of its orchestrator, which workers report to with SendMessage, its repositories with each one's name, path and trust, where its instructions file is, and its sessions as session_list gives them. Call it before each new task.",
            json!({}),
            &[],
            true,
            false,
        ),
    ]
}

/// What `repo` says of itself.
fn repo() -> Value {
    json!({ "type": "string", "description": "A repository of your space, by its name from space_status: needed from a space's orchestrator; any other session may name only its own repository." })
}

/// What `with_dirs` says of itself.
fn with_dirs() -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "maxItems": 8, "description": "For a space's orchestrator only: up to 8 more directories the new session may read, each a trusted repository's root or one of its linked worktrees in your space, by absolute path, as worktree_list and space_status give them." })
}

/// What `model` says of itself (ADR 0121, "A start's model, as amended").
fn model() -> Value {
    json!({
        "type": "string",
        "maxLength": MAX_MODEL,
        "pattern": "^[A-Za-z0-9._:/@\\[\\]][A-Za-z0-9._:/@\\[\\]-]*$",
        "description": "The model the new session's claude runs, passed as --model: an alias such as sonnet, opus, haiku or fable, or a full model id such as claude-sonnet-5-5 or claude-opus-5-5[1m]; the person's settings' own if not given. ASCII letters, digits and ._:/@[]-, not starting with -, at most 256 bytes. It is the session's own: its later processes start with it, until /model changes it in a native session.",
    })
}

/// What `mode` says of itself (ADR 0121, "A start's mode, as amended").
fn mode() -> Value {
    json!({
        "type": "string",
        "enum": ["native", "tui"],
        "description": "How the new session runs: native, claude -p shown in oso's own buffer, or tui, Claude Code's terminal UI in a terminal buffer; the person's [sessions] mode if not given. A native session starts only in a trusted project and with a brief; asked otherwise, it starts as tui, and the reply's mode and note say so. It is the session's own: its resumes and oso-restart run it the same way.",
    })
}

/// The session tools that act.
pub(super) fn acting() -> Vec<Value> {
    let mut tools = vec![
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
            "Start a new agent session in a new worktree of your session's repository, or of `repo` for a space's orchestrator, on a branch, with a brief as its first prompt. The person is asked first, unless their configuration lets your session start sessions without asking; the call returns once the session starts, and its `asked` says which happened. The brief gets the hand-off facts and a line saying to report back to you. Use this, never EnterWorktree or git worktree add.",
            json!({
                "branch": { "type": "string", "description": "The branch: an existing local one, or a new one." },
                "base": { "type": "string", "description": "Where a new branch starts; the commit your session is on if not given." },
                "brief": { "type": "string", "description": "The task for the new session, at most 64 KiB." },
                "name": { "type": "string", "description": "The session's name; the worktree's if not given." },
                "repo": repo(),
                "with_dirs": with_dirs(),
                "model": model(),
                "mode": mode(),
            }),
            &["branch", "brief"],
            false,
            false,
        ),
        tool(
            "session_start",
            "Start a new agent session in your session's repository, or one of its linked worktrees (`repo`'s for a space's orchestrator), with an optional brief as its first prompt. The person is asked first, unless their configuration lets your session start sessions without asking; the call returns once the session starts, and its `asked` says which happened.",
            json!({
                "directory": { "type": "string", "description": "An absolute path: the repository's root or a linked worktree's; your session's if not given, or `repo`'s root for a space's orchestrator." },
                "name": { "type": "string" },
                "brief": { "type": "string", "description": "The task for the new session, at most 64 KiB." },
                "repo": repo(),
                "with_dirs": with_dirs(),
                "model": model(),
                "mode": mode(),
            }),
            &[],
            false,
            false,
        ),
        tool(
            "draft_set",
            "Give oso the draft it asked you for: the commit message whose request named this id. It goes into the person's commit message buffer, their own text kept below it as comment lines; nothing is committed, the person reads it and commits. Only the message whose request oso pasted into your session, its newest, within 15 minutes, once.",
            json!({
                "kind": { "type": "string", "enum": ["commit"], "description": "What the draft is: a commit message." },
                "id": { "type": "string", "description": "The id oso's request named." },
                "text": { "type": "string", "description": "The message: its subject line, a blank line and its body, at most 16 KiB." },
            }),
            &["kind", "id", "text"],
            false,
            false,
        ),
        tool(
            "move_to_space",
            "Only for a session started outside oso, when the person asks you to move to one of oso's spaces: oso asks the person, then this conversation goes on in oso, in that space, once this process has ended. On success, end your turn at once with the one line the reply gives, and call no more tools. Two processes must never run one conversation: do not resume it anywhere yourself.",
            json!({
                "space": { "type": "string", "description": "The open space's slug: lower-case ASCII, its directory's name under the spaces root, which space-switch shows beside the name the person sees. Ask the person for it." },
            }),
            &["space"],
            false,
            false,
        ),
    ];
    tools.push(resume());
    tools.push(rename());
    tools.push(add_repo());
    tools.push(archive());
    tools
}

/// `session_resume`, which resumes a stopped session of the caller's space (ADR 0153).
fn resume() -> Value {
    tool(
        "session_resume",
        "For a space's orchestrator: resume a stopped or failed session of your space, by its id or name from session_list, in its own buffer, under its name and in its directory, with its conversation (claude --resume). It is refused while that conversation runs in another process: stop it there first; oso never forks it for you. The person is asked first, unless their configuration lets your session start sessions without asking; the call returns once the session runs again, and its `asked` says which happened. It sends the session no prompt: message it with SendMessage once it runs. The person resumes your space's earlier orchestrators and archived sessions themselves.",
        json!({
            "session": { "type": "string", "description": "A stopped session's id or name, from session_list." },
        }),
        &["session"],
        false,
        false,
    )
}

/// `session_archive`, which archives a session's row (ADR 0179).
fn archive() -> Value {
    tool(
        "session_archive",
        "Archive an agent session in oso, as the person's A in *Spaces* does: its row goes under Archived, where the person brings it back. A space's orchestrator archives its space's sessions; any session the sessions it started. Never your own session, an orchestrator, current or earlier, a pinned or a starting one. A stopped session is archived with no question. One that runs is stopped first, only when it is idle, done or ready for review and Claude Code's registry says its process is idle; a background shell it runs keeps it running, and the refusal says why. Stopping one asks the person first, unless their configuration lets your session start sessions without asking; the reply's `stopped` and `asked` say what happened. Its worktree and branch are never touched: worktree_remove removes a finished worktree.",
        json!({
            "session": { "type": "string", "description": "A session's id or name, from session_list." },
        }),
        &["session"],
        false,
        false,
    )
}

/// `space_add_repo`, which adds a repository to the caller's space (ADR 0161).
fn add_repo() -> Value {
    tool(
        "space_add_repo",
        "For a space's orchestrator: add a git repository to your space, by the absolute real path of its main checkout (no link, no worktree). The person is asked first, unless their configuration lets your session start sessions without asking and the repository is one oso already knows and trusts; a repository oso does not trust always asks, and joins restricted: no worktrees, workers or --add-dir until the person trusts it. The call returns once it is added, and its `asked` says which happened. Your running claude cannot read the new repository until /add-dir runs in it, or, run natively, until it is resumed with it: oso does either itself once your turn ends, asking no one, and the reply's `add_dir` and `note` say which.",
        json!({
            "path": { "type": "string", "description": "The repository's main checkout, an absolute path with no link in it." },
        }),
        &["path"],
        false,
        false,
    )
}

/// `session_rename`, which acts on a session's name only (ADR 0155).
fn rename() -> Value {
    tool(
        "session_rename",
        "Rename an agent session in oso: its name in session_list and *Spaces*, and its buffer, *claude: NAME*. You may rename your own session and the sessions you started; a space's orchestrator also its space's sessions. A space's orchestrator, current or earlier, is never renamed, as the space's sessions message it by the space's name. The name is ASCII letters, digits, `.`, `_` and `-`, at most 48 bytes, and no other session's or space's, case aside. A running session's claude keeps the name SendMessage knows it by (messaging_name) until it is resumed; the reply says so. Nothing is asked and nothing runs.",
        json!({
            "session": { "type": "string", "description": "A session's id or name, from session_list." },
            "name": { "type": "string", "description": "Its new name." },
        }),
        &["session", "name"],
        false,
        false,
    )
}
