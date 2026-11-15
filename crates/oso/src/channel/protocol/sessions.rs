//! The agent channel's session methods (ADR 0121, "The bridge and the channel"), one for each
//! session tool of `oso mcp`, parsed into plain data. What each may do, and for whom, is the
//! sessions module's to say; this only reads what was sent.
//!
//! Must not depend on: sockets, threads, editor state or the sessions module.
//!
//! - `session_list`: `{"offset": 0}` lists the sessions from the first.
//! - `session_state`: `{"session": "ID or name"}`.
//! - `worktree_list`: `{"repo": "api"}`, the linked worktrees of the caller's repository, or of
//!   the space's member `repo` for a space's orchestrator (ADR 0136, step 4).
//! - `worktree_remove`: `{"path": "/abs/repo/.claude/worktrees/x"}`.
//! - `start_worktree_session`: `{"branch": "x", "base": "main", "brief": "…", "name": "x",
//!   "repo": "api", "with_dirs": ["/abs/web"]}`; `base`, `name`, `repo` and `with_dirs` are
//!   optional.
//! - `session_start`: `{"directory": "/abs/repo", "name": "x", "brief": "…", "repo": "api",
//!   "with_dirs": ["/abs/web"]}`, each optional.
//! - `with_dirs`: at most [`MAX_WITH_DIRS`] absolute paths the new session also reads (ADR
//!   0136, step 4b).
//! - `space_status`: the caller's space, its repositories and its sessions.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value};

use super::{MAX_PATH, only, short};

/// The methods of this module, as the tools that call them are named.
pub const METHODS: [&str; 7] = [
    "session_list",
    "session_state",
    "worktree_list",
    "worktree_remove",
    "start_worktree_session",
    "session_start",
    "space_status",
];

/// The longest brief a session takes, in bytes, as the person's `*Brief*` (ADR 0121).
pub const MAX_BRIEF: usize = 64 * 1024;
/// The longest branch, base or session name taken, in bytes.
pub const MAX_NAME: usize = 255;
/// The most directories a call's `with_dirs` names.
pub const MAX_WITH_DIRS: usize = 8;

/// How long the app lets a worktree listing or a removal run before it replies with what it
/// has: their jobs run git, and `gh` for a listing, each call bounded, and may wait for a
/// thread (ADR 0121).
pub const JOB_ANSWER_WITHIN: Duration = Duration::from_secs(60);
/// How long the server waits for that reply: the app's limit and a margin.
pub const JOB_REPLY_WITHIN: Duration = Duration::from_secs(JOB_ANSWER_WITHIN.as_secs() + 5);

/// A session method's call.
#[derive(Clone, PartialEq, Eq)]
pub enum Call {
    List {
        offset: usize,
    },
    State {
        session: String,
    },
    WorktreeList {
        repo: Option<String>,
    },
    WorktreeRemove {
        path: PathBuf,
    },
    StartWorktree {
        branch: String,
        base: Option<String>,
        brief: String,
        name: Option<String>,
        repo: Option<String>,
        with_dirs: Vec<PathBuf>,
    },
    Start {
        directory: Option<PathBuf>,
        name: Option<String>,
        brief: Option<String>,
        repo: Option<String>,
        with_dirs: Vec<PathBuf>,
    },
    SpaceStatus,
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
            Self::WorktreeList { .. } => "worktree_list",
            Self::WorktreeRemove { .. } => "worktree_remove",
            Self::StartWorktree { .. } => "start_worktree_session",
            Self::Start { .. } => "session_start",
            Self::SpaceStatus => "space_status",
        }
    }

    /// How long the server holds its reply.
    pub fn hold(&self) -> Hold {
        match self {
            Self::List { .. } | Self::State { .. } | Self::SpaceStatus => Hold::Usual,
            Self::WorktreeList { .. } | Self::WorktreeRemove { .. } => Hold::Job(JOB_REPLY_WITHIN),
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
        "worktree_list" => only(params, method, &["repo"])
            .and_then(|()| optional(params, method, "repo", MAX_NAME))
            .map(|repo| Call::WorktreeList { repo }),
        "worktree_remove" => only(params, method, &["path"])
            .and_then(|()| path(params, method, "path"))
            .and_then(|path| path.ok_or_else(|| format!("{method} needs a \"path\"")))
            .map(|path| Call::WorktreeRemove { path }),
        "start_worktree_session" => parse_start_worktree(params),
        "session_start" => parse_start(params),
        "space_status" => only(params, method, &[]).map(|()| Call::SpaceStatus),
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
    only(
        params,
        method,
        &["branch", "base", "brief", "name", "repo", "with_dirs"],
    )?;
    Ok(Call::StartWorktree {
        branch: text(params, method, "branch", MAX_NAME)?,
        base: optional(params, method, "base", MAX_NAME)?,
        brief: brief(params, method)?.ok_or("start_worktree_session needs a \"brief\"")?,
        name: optional(params, method, "name", MAX_NAME)?,
        repo: optional(params, method, "repo", MAX_NAME)?,
        with_dirs: with_dirs(params, method)?,
    })
}

fn parse_start(params: &Map<String, Value>) -> Result<Call, String> {
    let method = "session_start";
    only(
        params,
        method,
        &["directory", "name", "brief", "repo", "with_dirs"],
    )?;
    Ok(Call::Start {
        directory: path(params, method, "directory")?,
        name: optional(params, method, "name", MAX_NAME)?,
        brief: brief(params, method)?,
        repo: optional(params, method, "repo", MAX_NAME)?,
        with_dirs: with_dirs(params, method)?,
    })
}

/// `with_dirs` of `method`: at most [`MAX_WITH_DIRS`] absolute paths, none twice; none when
/// not given.
fn with_dirs(params: &Map<String, Value>, method: &str) -> Result<Vec<PathBuf>, String> {
    let items = match params.get("with_dirs") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(format!("{method}'s \"with_dirs\" must be a list of paths")),
    };
    if items.len() > MAX_WITH_DIRS {
        return Err(format!(
            "{method}'s \"with_dirs\" names at most {MAX_WITH_DIRS} directories"
        ));
    }
    let mut dirs: Vec<PathBuf> = Vec::with_capacity(items.len());
    for item in items {
        let text = match item {
            Value::String(text) if !text.is_empty() && text.len() <= MAX_PATH => text,
            _ => {
                return Err(format!(
                    "{method}'s \"with_dirs\" holds a path that is no path"
                ));
            }
        };
        let dir = PathBuf::from(text);
        if !dir.is_absolute() {
            return Err(format!("{method}'s \"with_dirs\" paths must be absolute"));
        }
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    Ok(dirs)
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
            // Absolute on every OS: `/r` is not on Windows.
            call(
                "worktree_remove",
                json!({ "path": std::env::temp_dir().join("x").display().to_string() }),
            ),
            call(
                "start_worktree_session",
                json!({ "branch": "x", "brief": "do it" }),
            ),
            call("session_start", json!({})),
            call("space_status", json!({})),
        ];
        for (call, method) in calls.into_iter().zip(METHODS) {
            assert_eq!(call.unwrap().name(), method);
        }
        assert!(parse("state", &Map::new()).is_none());
    }

    #[test]
    fn a_repository_of_a_space_is_named_by_repo() {
        let repo = |call: Call| match call {
            Call::WorktreeList { repo }
            | Call::StartWorktree { repo, .. }
            | Call::Start { repo, .. } => repo,
            _ => None,
        };
        let named = |method: &str, mut value: Value| {
            value["repo"] = json!("api");
            repo(call(method, value).unwrap())
        };
        assert_eq!(named("worktree_list", json!({})).as_deref(), Some("api"));
        let start = json!({ "branch": "x", "brief": "b" });
        assert_eq!(
            named("start_worktree_session", start).as_deref(),
            Some("api")
        );
        assert_eq!(named("session_start", json!({})).as_deref(), Some("api"));
        assert_eq!(repo(call("session_start", json!({})).unwrap()), None);
        assert!(call("worktree_list", json!({ "repo": "" })).is_err());
        let long = "r".repeat(MAX_NAME + 1);
        assert!(call("worktree_list", json!({ "repo": long })).is_err());
        assert!(call("space_status", json!({ "repo": "api" })).is_err());
        assert_eq!(call("space_status", json!({})).unwrap().hold(), Hold::Usual);
    }

    #[test]
    fn with_dirs_names_at_most_eight_absolute_directories() {
        let dirs = |call: Call| match call {
            Call::StartWorktree { with_dirs, .. } | Call::Start { with_dirs, .. } => with_dirs,
            _ => Vec::new(),
        };
        let abs = |name: &str| std::env::temp_dir().join(name).display().to_string();
        let start = |with: Value| call("session_start", json!({ "with_dirs": with }));
        assert_eq!(
            dirs(call("session_start", json!({})).unwrap()),
            Vec::<std::path::PathBuf>::new()
        );
        let two = dirs(start(json!([abs("a"), abs("b"), abs("a")])).unwrap());
        assert_eq!(two.len(), 2, "a path named twice is one");
        let worktree = json!({ "branch": "x", "brief": "b", "with_dirs": [abs("a")] });
        assert_eq!(
            dirs(call("start_worktree_session", worktree).unwrap()).len(),
            1
        );
        let nine: Vec<String> = (0..9).map(|n| abs(&n.to_string())).collect();
        assert!(start(json!(nine)).is_err());
        assert!(start(json!(["relative"])).is_err());
        assert!(start(json!([""])).is_err());
        assert!(start(json!("not a list")).is_err());
        assert!(start(json!([1])).is_err());
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
