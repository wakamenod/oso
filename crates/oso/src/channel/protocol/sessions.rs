//! The agent channel's session methods (ADR 0121, "The bridge and the channel"), one for each
//! session tool of `oso mcp`, parsed into plain data. What each may do, and for whom, is the
//! sessions module's to say; this only reads what was sent.
//!
//! Must not depend on: sockets, threads, editor state or the sessions module.
//!
//! - `session_list`: `{"offset": 0}` lists the sessions from the first.
//! - `session_state`: `{"session": "ID or name"}`.
//! - `worktree_list`: the linked worktrees of the caller's repository.
//! - `worktree_remove`: `{"path": "/abs/repo/.claude/worktrees/x"}`.
//! - `start_worktree_session`: `{"branch": "x", "base": "main", "brief": "…", "name": "x"}`;
//!   `base` and `name` are optional.
//! - `session_start`: `{"directory": "/abs/repo", "name": "x", "brief": "…"}`, each optional.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value};

use super::{MAX_PATH, only, short};

/// The methods of this module, as the tools that call them are named.
pub const METHODS: [&str; 6] = [
    "session_list",
    "session_state",
    "worktree_list",
    "worktree_remove",
    "start_worktree_session",
    "session_start",
];

/// The longest brief a session takes, in bytes, as the person's `*Brief*` (ADR 0121).
pub const MAX_BRIEF: usize = 64 * 1024;
/// The longest branch, base or session name taken, in bytes.
pub const MAX_NAME: usize = 255;

/// How long the server waits for the UI's reply to a worktree listing or a removal: their
/// jobs run git, and `gh` for a listing, each call bounded (ADR 0121).
pub const JOB_REPLY_WITHIN: Duration = Duration::from_secs(65);

/// A session method's call.
#[derive(Clone, PartialEq, Eq)]
pub enum Call {
    List {
        offset: usize,
    },
    State {
        session: String,
    },
    WorktreeList,
    WorktreeRemove {
        path: PathBuf,
    },
    StartWorktree {
        branch: String,
        base: Option<String>,
        brief: String,
        name: Option<String>,
    },
    Start {
        directory: Option<PathBuf>,
        name: Option<String>,
        brief: Option<String>,
    },
}

/// How long the server holds a call's reply for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// As any request.
    Usual,
    /// Up to this long: its job's calls are bounded.
    Job(Duration),
    /// As long as the person takes to answer its question, as the connection's question waits
    /// (ADR 0121, the author's choice): the client going ends it.
    Person,
}

impl Call {
    /// The tool's name, which is its method's.
    pub fn name(&self) -> &'static str {
        match self {
            Self::List { .. } => "session_list",
            Self::State { .. } => "session_state",
            Self::WorktreeList => "worktree_list",
            Self::WorktreeRemove { .. } => "worktree_remove",
            Self::StartWorktree { .. } => "start_worktree_session",
            Self::Start { .. } => "session_start",
        }
    }

    /// How long the server holds its reply.
    pub fn hold(&self) -> Hold {
        match self {
            Self::List { .. } | Self::State { .. } => Hold::Usual,
            Self::WorktreeList | Self::WorktreeRemove { .. } => Hold::Job(JOB_REPLY_WITHIN),
            Self::StartWorktree { .. } | Self::Start { .. } => Hold::Person,
        }
    }
}

/// Its `Debug` gives a brief's length only: a brief is the agent's text, and is never logged.
impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let brief = |brief: Option<&String>| brief.map(|text| format!("{} bytes", text.len()));
        match self {
            Self::StartWorktree { brief: text, .. } => f
                .debug_struct("StartWorktree")
                .field("brief", &brief(Some(text)))
                .finish_non_exhaustive(),
            Self::Start { brief: text, .. } => f
                .debug_struct("Start")
                .field("brief", &brief(text.as_ref()))
                .finish_non_exhaustive(),
            other => f.write_str(other.name()),
        }
    }
}

/// The call of `method`, if it is a session method.
pub fn parse(method: &str, params: &Map<String, Value>) -> Option<Result<Call, String>> {
    Some(match method {
        "session_list" => parse_list(params),
        "session_state" => only(params, method, &["session"])
            .and_then(|()| text(params, method, "session", MAX_NAME))
            .map(|session| Call::State { session }),
        "worktree_list" => only(params, method, &[]).map(|()| Call::WorktreeList),
        "worktree_remove" => only(params, method, &["path"])
            .and_then(|()| path(params, method, "path"))
            .and_then(|path| path.ok_or_else(|| format!("{method} needs a \"path\"")))
            .map(|path| Call::WorktreeRemove { path }),
        "start_worktree_session" => parse_start_worktree(params),
        "session_start" => parse_start(params),
        _ => return None,
    })
}

fn parse_list(params: &Map<String, Value>) -> Result<Call, String> {
    only(params, "session_list", &["offset"])?;
    let offset = match params.get("offset") {
        None | Some(Value::Null) => 0,
        Some(offset) => offset
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("session_list's \"offset\" must be a whole number")?,
    };
    Ok(Call::List { offset })
}

fn parse_start_worktree(params: &Map<String, Value>) -> Result<Call, String> {
    let method = "start_worktree_session";
    only(params, method, &["branch", "base", "brief", "name"])?;
    Ok(Call::StartWorktree {
        branch: text(params, method, "branch", MAX_NAME)?,
        base: optional(params, method, "base", MAX_NAME)?,
        brief: brief(params, method)?.ok_or("start_worktree_session needs a \"brief\"")?,
        name: optional(params, method, "name", MAX_NAME)?,
    })
}

fn parse_start(params: &Map<String, Value>) -> Result<Call, String> {
    let method = "session_start";
    only(params, method, &["directory", "name", "brief"])?;
    Ok(Call::Start {
        directory: path(params, method, "directory")?,
        name: optional(params, method, "name", MAX_NAME)?,
        brief: brief(params, method)?,
    })
}

/// The string `name` of `method`, of at most `max` bytes and not empty.
fn text(
    params: &Map<String, Value>,
    method: &str,
    name: &str,
    max: usize,
) -> Result<String, String> {
    optional(params, method, name, max)?
        .ok_or_else(|| format!("{method} needs a string {:?}", short(name)))
}

/// The string `name` of `method`, if given: at most `max` bytes, and not empty.
fn optional(
    params: &Map<String, Value>,
    method: &str,
    name: &str,
    max: usize,
) -> Result<Option<String>, String> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => {
            Err(format!("{method}'s {name:?} is empty"))
        }
        Some(Value::String(text)) if text.len() > max => {
            Err(format!("{method}'s {name:?} is at most {max} bytes"))
        }
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("{method}'s {name:?} must be a string")),
    }
}

/// The brief, if given: at most [`MAX_BRIEF`] bytes, and more than blank.
fn brief(params: &Map<String, Value>, method: &str) -> Result<Option<String>, String> {
    let brief = optional(params, method, "brief", MAX_BRIEF)?;
    if brief.as_deref().is_some_and(|text| text.trim().is_empty()) {
        return Err(format!("{method}'s \"brief\" is blank"));
    }
    Ok(brief)
}

/// The absolute path `name` of `method`, if given.
fn path(params: &Map<String, Value>, method: &str, name: &str) -> Result<Option<PathBuf>, String> {
    let Some(text) = optional(params, method, name, MAX_PATH)? else {
        return Ok(None);
    };
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err(format!("{method}'s {name:?} must be an absolute path"));
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::{Call, Hold, MAX_BRIEF, MAX_NAME, METHODS, parse};

    fn params(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => panic!("not an object"),
        }
    }

    fn call(method: &str, value: Value) -> Result<Call, String> {
        parse(method, &params(value)).expect("a session method")
    }

    #[test]
    fn each_method_parses_and_names_itself() {
        let calls = [
            call("session_list", json!({})),
            call("session_state", json!({ "session": "api" })),
            call("worktree_list", json!({})),
            call(
                "worktree_remove",
                json!({ "path": "/r/.claude/worktrees/x" }),
            ),
            call(
                "start_worktree_session",
                json!({ "branch": "x", "brief": "do it" }),
            ),
            call("session_start", json!({})),
        ];
        for (call, method) in calls.into_iter().zip(METHODS) {
            assert_eq!(call.unwrap().name(), method);
        }
        assert!(parse("state", &Map::new()).is_none());
    }

    #[test]
    fn arguments_are_checked_at_their_bounds() {
        let long = "b".repeat(MAX_NAME + 1);
        assert!(call("session_state", json!({ "session": long })).is_err());
        assert!(call("session_state", json!({})).is_err());
        assert!(call("session_state", json!({ "session": "" })).is_err());
        assert!(call("session_list", json!({ "offset": -1 })).is_err());
        assert!(call("worktree_list", json!({ "x": 1 })).is_err());
        assert!(call("worktree_remove", json!({ "path": "relative" })).is_err());
        assert!(call("worktree_remove", json!({})).is_err());
        let brief = "a".repeat(MAX_BRIEF);
        assert!(
            call(
                "start_worktree_session",
                json!({ "branch": "x", "brief": brief })
            )
            .is_ok()
        );
        let over = "a".repeat(MAX_BRIEF + 1);
        assert!(
            call(
                "start_worktree_session",
                json!({ "branch": "x", "brief": over })
            )
            .is_err()
        );
        assert!(call("start_worktree_session", json!({ "branch": "x" })).is_err());
        assert!(
            call(
                "start_worktree_session",
                json!({ "branch": "x", "brief": " \n" })
            )
            .is_err()
        );
        assert!(call("session_start", json!({ "directory": "r" })).is_err());
        assert!(call("session_start", json!({ "brief": 3 })).is_err());
    }

    #[test]
    fn only_asking_calls_wait_for_the_person_and_briefs_never_show() {
        let start = call(
            "start_worktree_session",
            json!({ "branch": "x", "brief": "secret plan" }),
        )
        .unwrap();
        assert_eq!(start.hold(), Hold::Person);
        assert!(!format!("{start:?}").contains("secret"));
        assert_eq!(call("session_list", json!({})).unwrap().hold(), Hold::Usual);
        assert!(matches!(
            call("worktree_list", json!({})).unwrap().hold(),
            Hold::Job(_)
        ));
    }
}
