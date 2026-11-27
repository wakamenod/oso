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
//!   "repo": "api", "with_dirs": ["/abs/web"], "model": "sonnet", "mode": "tui"}`; `base`,
//!   `name`, `repo`, `with_dirs`, `model` and `mode` are optional.
//! - `session_start`: `{"directory": "/abs/repo", "name": "x", "brief": "…", "repo": "api",
//!   "with_dirs": ["/abs/web"], "model": "sonnet", "mode": "tui"}`, each optional.
//! - `model`: the new session's `--model`, one word that [`is_model`] takes (ADR 0121, "A
//!   start's model, as amended").
//! - `mode`: `"native"` or `"tui"`, how the new session runs ([`RunMode`]; ADR 0121, "A
//!   start's mode, as amended").
//! - `with_dirs`: at most [`MAX_WITH_DIRS`] absolute paths the new session also reads (ADR
//!   0136, step 4b).
//! - `space_status`: the caller's space, its repositories and its sessions.
//! - `draft_set`: `{"kind": "commit", "id": "1a2b3c4d", "text": "…"}`, the draft of the commit
//!   message oso asked for with `id` (ADR 0127, step 6a).
//! - `move_to_space`: `{"space": "fix-login"}`, the open space a session started outside oso
//!   moves into (ADR 0151).
//! - `session_resume`: `{"session": "ID or name"}`, a stopped session of the caller's space
//!   (ADR 0153).
//! - `session_rename`: `{"session": "ID or name", "name": "api-v2"}`, the session's new name
//!   (ADR 0155).
//! - `space_add_repo`: `{"path": "/abs/web"}`, a repository's main checkout for the caller's
//!   space (ADR 0161).
//! - `session_archive`: `{"session": "ID or name"}`, a session's row to archive (ADR 0179).

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value};

use super::{MAX_PATH, only, short};

/// The methods of this module, as the tools that call them are named.
pub const METHODS: [&str; 13] = [
    "session_list",
    "session_state",
    "worktree_list",
    "worktree_remove",
    "start_worktree_session",
    "session_start",
    "space_status",
    "draft_set",
    "move_to_space",
    "session_resume",
    "session_rename",
    "space_add_repo",
    "session_archive",
];

/// The longest brief a session takes, in bytes, as the person's `*Brief*` (ADR 0121).
pub const MAX_BRIEF: usize = 64 * 1024;
/// The longest branch, base or session name taken, in bytes.
pub const MAX_NAME: usize = 255;
/// The longest draft taken, in bytes (ADR 0127, step 6a).
pub const MAX_DRAFT: usize = 16 * 1024;
/// The longest space name taken, in bytes, as a space's name is (ADR 0136).
pub const MAX_SPACE_NAME: usize = 48;
/// The longest request id taken, in bytes.
const MAX_ID: usize = 64;
/// The most directories a call's `with_dirs` names.
pub const MAX_WITH_DIRS: usize = 8;
/// The longest model taken, in bytes, as a native session's choice keeps one.
pub const MAX_MODEL: usize = 256;

/// How long the app lets a worktree listing or a removal run before it replies with what it
/// has: their jobs run git, and `gh` for a listing, each call bounded but a removal's own
/// `git worktree remove`, and may wait for a thread (ADR 0121).
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
        /// Its `--model`, which [`is_model`] took.
        model: Option<String>,
        /// How it runs, if the call says; else as `[sessions] mode` says.
        mode: Option<RunMode>,
    },
    Start {
        directory: Option<PathBuf>,
        name: Option<String>,
        brief: Option<String>,
        repo: Option<String>,
        with_dirs: Vec<PathBuf>,
        /// Its `--model`, which [`is_model`] took.
        model: Option<String>,
        /// How it runs, if the call says; else as `[sessions] mode` says.
        mode: Option<RunMode>,
    },
    SpaceStatus,
    /// The draft of a commit's message, for the request `id` (`kind` is always `commit`).
    DraftSet {
        id: String,
        text: String,
    },
    /// A session started outside oso asks to move into the open space named `space` (ADR
    /// 0151).
    MoveToSpace {
        space: String,
    },
    /// A stopped session to resume, by its id or its name (ADR 0153).
    Resume {
        session: String,
    },
    /// A session to rename, by its id or its name, and its new name (ADR 0155).
    Rename {
        session: String,
        name: String,
    },
    /// A repository's main checkout, by its absolute path, to add to the caller's space
    /// (ADR 0161).
    SpaceAddRepo {
        path: PathBuf,
    },
    /// A session to archive, by its id or its name (ADR 0179).
    Archive {
        session: String,
    },
}

/// How a session a tool starts runs (ADR 0121, "A start's mode, as amended").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunMode {
    /// `claude -p`, shown in oso's own buffer (ADR 0165).
    Native,
    /// Claude Code's terminal UI, in a terminal buffer.
    Tui,
}

impl RunMode {
    /// How a call names it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Tui => "tui",
        }
    }
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
            Self::DraftSet { .. } => "draft_set",
            Self::MoveToSpace { .. } => "move_to_space",
            Self::Resume { .. } => "session_resume",
            Self::Rename { .. } => "session_rename",
            Self::SpaceAddRepo { .. } => "space_add_repo",
            Self::Archive { .. } => "session_archive",
        }
    }

    /// How long the server holds its reply.
    pub fn hold(&self) -> Hold {
        match self {
            Self::List { .. }
            | Self::State { .. }
            | Self::SpaceStatus
            | Self::DraftSet { .. }
            | Self::Rename { .. } => Hold::Usual,
            Self::WorktreeList { .. } | Self::WorktreeRemove { .. } => Hold::Job(JOB_REPLY_WITHIN),
            // An archive that stops a session may ask (ADR 0179).
            Self::StartWorktree { .. }
            | Self::Start { .. }
            | Self::MoveToSpace { .. }
            | Self::Resume { .. }
            | Self::SpaceAddRepo { .. }
            | Self::Archive { .. } => Hold::Person,
        }
    }
}

/// Its `Debug` gives a brief's or a draft's length only: they are the agent's text, and are
/// never logged.
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
            Self::DraftSet { text, .. } => f
                .debug_struct("DraftSet")
                .field("text", &brief(Some(text)))
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
        "draft_set" => parse_draft(params),
        "move_to_space" => only(params, method, &["space"])
            .and_then(|()| text(params, method, "space", MAX_SPACE_NAME))
            .map(|space| Call::MoveToSpace { space }),
        "session_resume" => only(params, method, &["session"])
            .and_then(|()| text(params, method, "session", MAX_NAME))
            .map(|session| Call::Resume { session }),
        "session_rename" => only(params, method, &["session", "name"]).and_then(|()| {
            Ok(Call::Rename {
                session: text(params, method, "session", MAX_NAME)?,
                name: text(params, method, "name", MAX_NAME)?,
            })
        }),
        "space_add_repo" => only(params, method, &["path"])
            .and_then(|()| path(params, method, "path"))
            .and_then(|path| path.ok_or_else(|| format!("{method} needs a \"path\"")))
            .map(|path| Call::SpaceAddRepo { path }),
        "session_archive" => only(params, method, &["session"])
            .and_then(|()| text(params, method, "session", MAX_NAME))
            .map(|session| Call::Archive { session }),
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

fn parse_draft(params: &Map<String, Value>) -> Result<Call, String> {
    let method = "draft_set";
    only(params, method, &["kind", "id", "text"])?;
    if text(params, method, "kind", MAX_NAME)? != "commit" {
        return Err("draft_set's \"kind\" is \"commit\", the one kind oso asks for".to_owned());
    }
    Ok(Call::DraftSet {
        id: text(params, method, "id", MAX_ID)?,
        text: text(params, method, "text", MAX_DRAFT)?,
    })
}

fn parse_start_worktree(params: &Map<String, Value>) -> Result<Call, String> {
    let method = "start_worktree_session";
    only(
        params,
        method,
        &[
            "branch",
            "base",
            "brief",
            "name",
            "repo",
            "with_dirs",
            "model",
            "mode",
        ],
    )?;
    Ok(Call::StartWorktree {
        branch: text(params, method, "branch", MAX_NAME)?,
        base: optional(params, method, "base", MAX_NAME)?,
        brief: brief(params, method)?.ok_or("start_worktree_session needs a \"brief\"")?,
        name: optional(params, method, "name", MAX_NAME)?,
        repo: optional(params, method, "repo", MAX_NAME)?,
        with_dirs: with_dirs(params, method)?,
        model: model(params, method)?,
        mode: mode(params, method)?,
    })
}

fn parse_start(params: &Map<String, Value>) -> Result<Call, String> {
    let method = "session_start";
    only(
        params,
        method,
        &[
            "directory",
            "name",
            "brief",
            "repo",
            "with_dirs",
            "model",
            "mode",
        ],
    )?;
    Ok(Call::Start {
        directory: path(params, method, "directory")?,
        name: optional(params, method, "name", MAX_NAME)?,
        brief: brief(params, method)?,
        repo: optional(params, method, "repo", MAX_NAME)?,
        with_dirs: with_dirs(params, method)?,
        model: model(params, method)?,
        mode: mode(params, method)?,
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

/// Whether `text` can be a session's `--model`: not empty, at most [`MAX_MODEL`] bytes, only
/// ASCII letters, digits and `._:/@[]-`, and no `-` first. An alias (`sonnet`, `opus`), an id
/// (`claude-opus-5-5[1m]`) or a provider's (`us.anthropic.…-v1:0`, an ARN) reads so; no space,
/// quote, `=`, control, invisible or look-alike character does. It goes on the line as one
/// argument, `--model=TEXT`, so it can never be taken for an option either way.
pub fn is_model(text: &str) -> bool {
    let fits = |b: u8| b.is_ascii_alphanumeric() || b"._:/@[]-".contains(&b);
    !text.is_empty() && text.len() <= MAX_MODEL && !text.starts_with('-') && text.bytes().all(fits)
}

/// `model` of `method`, if given: one that [`is_model`] takes.
fn model(params: &Map<String, Value>, method: &str) -> Result<Option<String>, String> {
    let model = optional(params, method, "model", MAX_MODEL)?;
    if model.as_deref().is_some_and(|text| !is_model(text)) {
        return Err(format!(
            "{method}'s \"model\" must be a model's alias or id: ASCII letters, digits and ._:/@[]-, not starting with -"
        ));
    }
    Ok(model)
}

/// `mode` of `method`, if given: `"native"` or `"tui"`.
fn mode(params: &Map<String, Value>, method: &str) -> Result<Option<RunMode>, String> {
    match params.get("mode") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text == "native" => Ok(Some(RunMode::Native)),
        Some(Value::String(text)) if text == "tui" => Ok(Some(RunMode::Tui)),
        Some(_) => Err(format!("{method}'s \"mode\" must be \"native\" or \"tui\"")),
    }
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

    use super::{Call, Hold, MAX_BRIEF, MAX_NAME, METHODS, RunMode, parse};

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
            call(
                "draft_set",
                json!({ "kind": "commit", "id": "1a2b3c4d", "text": "Fix it" }),
            ),
            call("move_to_space", json!({ "space": "fix-login" })),
            call("session_resume", json!({ "session": "api" })),
            call(
                "session_rename",
                json!({ "session": "api", "name": "api-v2" }),
            ),
            call(
                "space_add_repo",
                json!({ "path": std::env::temp_dir().join("web").display().to_string() }),
            ),
            call("session_archive", json!({ "session": "api" })),
        ];
        assert_eq!(calls.len(), METHODS.len());
        for (call, method) in calls.into_iter().zip(METHODS) {
            assert_eq!(call.unwrap().name(), method);
        }
        assert!(parse("state", &Map::new()).is_none());
    }

    #[test]
    fn a_draft_is_a_commit_message_of_at_most_16_kib_with_its_id() {
        let draft = |value: Value| call("draft_set", value);
        let set = draft(json!({ "kind": "commit", "id": "ab", "text": "Fix it" })).unwrap();
        assert_eq!(
            set,
            Call::DraftSet {
                id: "ab".into(),
                text: "Fix it".into()
            }
        );
        assert_eq!(set.hold(), Hold::Usual);
        // Its text is never logged.
        assert!(!format!("{set:?}").contains("Fix it"));
        assert!(draft(json!({ "kind": "pr", "id": "ab", "text": "x" })).is_err());
        assert!(draft(json!({ "id": "ab", "text": "x" })).is_err());
        assert!(draft(json!({ "kind": "commit", "text": "x" })).is_err());
        assert!(draft(json!({ "kind": "commit", "id": "ab", "text": "" })).is_err());
        assert!(draft(json!({ "kind": "commit", "id": "ab", "text": "x", "y": 1 })).is_err());
        let long = "x".repeat(super::MAX_DRAFT + 1);
        assert!(draft(json!({ "kind": "commit", "id": "ab", "text": long })).is_err());
        let full = "x".repeat(super::MAX_DRAFT);
        assert!(draft(json!({ "kind": "commit", "id": "ab", "text": full })).is_ok());
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
    fn a_model_is_one_word_of_a_models_characters_within_bounds() {
        let model = |call: Call| match call {
            Call::StartWorktree { model, .. } | Call::Start { model, .. } => model,
            _ => None,
        };
        let start = |value: Value| call("session_start", json!({ "model": value }));
        let worktree = |value: Value| {
            call(
                "start_worktree_session",
                json!({ "branch": "x", "brief": "b", "model": value }),
            )
        };
        assert_eq!(model(call("session_start", json!({})).unwrap()), None);
        for good in [
            "sonnet",
            "opus",
            "haiku",
            "fable",
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "us.anthropic.claude-sonnet-5-5-v1:0",
            "arn:aws:bedrock:us-east-1:1:inference-profile/x",
            "claude@2026",
        ] {
            assert_eq!(model(start(json!(good)).unwrap()).as_deref(), Some(good));
            assert_eq!(model(worktree(json!(good)).unwrap()).as_deref(), Some(good));
        }
        let longest = "m".repeat(super::MAX_MODEL);
        assert!(start(json!(longest)).is_ok());
        let over = "m".repeat(super::MAX_MODEL + 1);
        for bad in [
            json!(""),
            json!(over),
            json!("-p"),
            json!("--dangerously-skip-permissions"),
            json!("sonnet opus"),
            json!("sonnet\n"),
            json!("so;rm"),
            json!("$(x)"),
            json!("a=b"),
            json!("o\u{2064}pus"),
            json!("ｏｐｕｓ"),
            json!(3),
            json!(["opus"]),
        ] {
            assert!(start(bad.clone()).is_err(), "{bad}");
            assert!(worktree(bad.clone()).is_err(), "{bad}");
        }
        let why = start(json!("-p")).unwrap_err();
        assert!(why.contains("\"model\""), "{why}");
    }

    #[test]
    fn a_mode_is_native_or_tui_and_none_when_not_given() {
        let mode = |call: Call| match call {
            Call::StartWorktree { mode, .. } | Call::Start { mode, .. } => mode,
            _ => None,
        };
        let start = |value: Value| call("session_start", json!({ "mode": value }));
        let worktree = |value: Value| {
            call(
                "start_worktree_session",
                json!({ "branch": "x", "brief": "b", "mode": value }),
            )
        };
        assert_eq!(mode(call("session_start", json!({})).unwrap()), None);
        for (said, run) in [("native", RunMode::Native), ("tui", RunMode::Tui)] {
            assert_eq!(mode(start(json!(said)).unwrap()), Some(run));
            assert_eq!(mode(worktree(json!(said)).unwrap()), Some(run));
            assert_eq!(run.name(), said);
        }
        for bad in [
            json!(""),
            json!("Native"),
            json!("TUI"),
            json!("terminal"),
            json!(1),
            json!(true),
        ] {
            assert!(start(bad.clone()).is_err(), "{bad}");
            assert!(worktree(bad.clone()).is_err(), "{bad}");
        }
        let why = start(json!("auto")).unwrap_err();
        assert!(why.contains("\"mode\""), "{why}");
    }

    #[test]
    fn a_rename_names_a_session_and_a_name_each_within_bounds() {
        let rename = |value: Value| call("session_rename", value);
        let name = "n".repeat(MAX_NAME);
        let long = "n".repeat(MAX_NAME + 1);
        assert!(rename(json!({ "session": "x", "name": name })).is_ok());
        assert!(rename(json!({ "session": "x", "name": long })).is_err());
        assert!(rename(json!({ "session": long, "name": "y" })).is_err());
        assert!(rename(json!({ "session": "x" })).is_err());
        assert!(rename(json!({ "name": "y" })).is_err());
        assert!(rename(json!({ "session": "x", "name": "" })).is_err());
        assert!(rename(json!({ "session": "x", "name": "y", "z": 1 })).is_err());
    }

    #[test]
    fn an_archive_names_one_session_within_bounds() {
        let archive = |value: Value| call("session_archive", value);
        assert!(archive(json!({ "session": "n".repeat(MAX_NAME) })).is_ok());
        assert!(archive(json!({ "session": "n".repeat(MAX_NAME + 1) })).is_err());
        assert!(archive(json!({})).is_err());
        assert!(archive(json!({ "session": "" })).is_err());
        assert!(archive(json!({ "session": "x", "stop": true })).is_err());
    }

    #[test]
    fn arguments_are_checked_at_their_bounds() {
        let long = "b".repeat(MAX_NAME + 1);
        assert!(call("session_state", json!({ "session": long })).is_err());
        assert!(call("session_state", json!({})).is_err());
        assert!(call("session_state", json!({ "session": "" })).is_err());
        let long = "b".repeat(MAX_NAME + 1);
        assert!(call("session_resume", json!({ "session": long })).is_err());
        assert!(call("session_resume", json!({})).is_err());
        assert!(call("session_resume", json!({ "session": "x", "brief": "y" })).is_err());
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
        let space = "s".repeat(super::MAX_SPACE_NAME);
        assert!(call("move_to_space", json!({ "space": space })).is_ok());
        let over = "s".repeat(super::MAX_SPACE_NAME + 1);
        assert!(call("move_to_space", json!({ "space": over })).is_err());
        assert!(call("move_to_space", json!({})).is_err());
        assert!(call("move_to_space", json!({ "space": "" })).is_err());
        assert!(call("move_to_space", json!({ "space": "x", "pid": 1 })).is_err());
    }

    #[test]
    fn an_added_repository_is_one_absolute_path_within_bounds() {
        let abs = std::env::temp_dir().join("web").display().to_string();
        assert!(call("space_add_repo", json!({ "path": abs })).is_ok());
        assert!(call("space_add_repo", json!({ "path": "web" })).is_err());
        assert!(call("space_add_repo", json!({})).is_err());
        assert!(call("space_add_repo", json!({ "path": "" })).is_err());
        assert!(call("space_add_repo", json!({ "path": abs, "repo": "x" })).is_err());
        let long = format!("/{}", "p".repeat(super::MAX_PATH));
        assert!(call("space_add_repo", json!({ "path": long })).is_err());
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
        let resume = call("session_resume", json!({ "session": "api" })).unwrap();
        assert_eq!(resume.hold(), Hold::Person);
        assert_eq!(call("session_list", json!({})).unwrap().hold(), Hold::Usual);
        let rename = call("session_rename", json!({ "session": "a", "name": "b" })).unwrap();
        assert_eq!(rename.hold(), Hold::Usual, "it asks nothing");
        let archive = call("session_archive", json!({ "session": "a" })).unwrap();
        assert_eq!(archive.hold(), Hold::Person, "it may ask");
        let moving = call("move_to_space", json!({ "space": "x" })).unwrap();
        assert_eq!(moving.hold(), Hold::Person);
        let abs = std::env::temp_dir().join("web").display().to_string();
        let adding = call("space_add_repo", json!({ "path": abs })).unwrap();
        assert_eq!(adding.hold(), Hold::Person, "it may ask");
        assert!(matches!(
            call("worktree_list", json!({})).unwrap().hold(),
            Hold::Job(_)
        ));
    }
}
