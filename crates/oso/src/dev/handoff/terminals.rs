//! The terminals a state carries over `oso dev`'s restart (ADR 0120, step 11): what each
//! entry says beside its item, which holds the terminal's state in base64, and reading them
//! back, checking everything before anything changes. The masters do not travel in the state:
//! the supervisor carries them with `SCM_RIGHTS`, in the order of the entries.
//!
//! Must not depend on: `app`, `server`, sockets or the terminals themselves; these are plain
//! functions of what the app hands them.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};

use super::format::{Items, array, size};

/// The most terminals a restart carries: as many masters as one message passes.
pub const MAX_TERMINALS: usize = 64;

/// The longest buffer name, session name or path an entry gives.
const MAX_TEXT: usize = 4096;

/// How a terminal's state is written, as the entry says it.
const SNAPSHOT: &str = "snapshot";
const ESCAPES: &str = "escapes";

/// A terminal the state carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalState {
    /// The item holding its state, in base64.
    pub item: usize,
    pub snapshot: bool,
    /// Its buffer's name.
    pub name: String,
    pub rows: u16,
    pub columns: u16,
    pub pid: Option<u32>,
    /// When its child started, as the system said.
    pub started: Option<SystemTime>,
    /// Whether the person let agents read it (ADR 0120, Q16).
    pub agent_reads: bool,
    /// Whether keys go to it in char mode.
    pub char_mode: bool,
    /// The hosted session it runs, if any (ADR 0121).
    pub session: Option<SessionState>,
}

/// What the state says of a hosted session, so the new instance hosts it as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    /// Its id, which the app reads as a session's: a state only carries it.
    pub id: String,
    pub name: String,
    pub directory: PathBuf,
    pub root: Option<PathBuf>,
    pub parent: Option<String>,
    pub baseline: Option<String>,
    pub pinned: bool,
    pub changed: u64,
}

/// What an entry says of `terminal`, beside its item. An error if a path is not UTF-8.
pub fn entry(terminal: &TerminalState) -> Result<Value, String> {
    let session = match &terminal.session {
        None => Value::Null,
        Some(session) => json!({
            "id": session.id,
            "name": session.name,
            "directory": utf8(&session.directory)?,
            "root": session.root.as_deref().map(utf8).transpose()?,
            "parent": session.parent,
            "baseline": session.baseline,
            "pinned": session.pinned,
            "changed": session.changed,
        }),
    };
    Ok(json!({
        "item": terminal.item,
        "encoding": if terminal.snapshot { SNAPSHOT } else { ESCAPES },
        "name": terminal.name,
        "rows": terminal.rows,
        "columns": terminal.columns,
        "pid": terminal.pid,
        "started": terminal.started.and_then(micros),
        "agent_reads": terminal.agent_reads,
        "char_mode": terminal.char_mode,
        "session": session,
    }))
}

fn utf8(path: &std::path::Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "cannot hand over a session whose directory is not UTF-8".to_owned())
}

fn micros(at: SystemTime) -> Option<u64> {
    let since = at.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    u64::try_from(since.as_micros()).ok()
}

/// The terminals `state` carries, each claiming its item; none in a state from before.
pub(super) fn parse(
    state: &Map<String, Value>,
    items: &mut Items,
) -> Result<Vec<TerminalState>, String> {
    if !state.contains_key("terminals") {
        return Ok(Vec::new());
    }
    let entries = array(state, "terminals")?;
    if entries.len() > MAX_TERMINALS {
        return Err(format!("the state has more than {MAX_TERMINALS} terminals"));
    }
    entries
        .iter()
        .map(|entry| {
            let entry = entry.as_object().ok_or("a terminal is not an object")?;
            parse_entry(entry, items)
        })
        .collect()
}

fn parse_entry(entry: &Map<String, Value>, items: &mut Items) -> Result<TerminalState, String> {
    let item = items.claim(entry, None)?;
    let snapshot = match entry.get("encoding").and_then(Value::as_str) {
        Some(SNAPSHOT) => true,
        Some(ESCAPES) => false,
        _ => return Err("a terminal's encoding is neither snapshot nor escapes".into()),
    };
    let side = |name: &str| {
        size(entry, name).and_then(|n| {
            u16::try_from(n)
                .ok()
                .filter(|&n| n > 0)
                .ok_or_else(|| format!("a terminal's {name} is not a side"))
        })
    };
    let pid = match entry.get("pid") {
        None | Some(Value::Null) => None,
        Some(pid) => Some(
            pid.as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .filter(|&pid| pid > 1)
                .ok_or("a terminal's pid is not a process id")?,
        ),
    };
    let started = match entry.get("started") {
        None | Some(Value::Null) => None,
        Some(at) => Some(
            at.as_u64()
                .and_then(|at| SystemTime::UNIX_EPOCH.checked_add(Duration::from_micros(at)))
                .ok_or("a terminal's start is not a time")?,
        ),
    };
    Ok(TerminalState {
        item,
        snapshot,
        name: text(entry, "name")?.ok_or("a terminal has no name")?,
        rows: side("rows")?,
        columns: side("columns")?,
        pid,
        started,
        agent_reads: switch(entry, "agent_reads")?,
        char_mode: switch(entry, "char_mode")?,
        session: match entry.get("session") {
            None | Some(Value::Null) => None,
            Some(Value::Object(session)) => Some(parse_session(session)?),
            Some(_) => return Err("a terminal's session is not an object".into()),
        },
    })
}

fn parse_session(session: &Map<String, Value>) -> Result<SessionState, String> {
    // A session's id is 36 characters (ADR 0121); the app reads it as one.
    let id = |name: &str| -> Result<Option<String>, String> {
        match text(session, name)? {
            Some(id) if id.len() != 36 || !id.is_ascii() => {
                Err(format!("a session's {name} is not an id"))
            }
            other => Ok(other),
        }
    };
    let path = |name: &str| -> Result<Option<PathBuf>, String> {
        let path = text(session, name)?.map(PathBuf::from);
        match path {
            Some(path) if !path.is_absolute() => Err(format!("a session's {name} is not absolute")),
            other => Ok(other),
        }
    };
    Ok(SessionState {
        id: id("id")?.ok_or("a session has no id")?,
        name: text(session, "name")?.ok_or("a session has no name")?,
        directory: path("directory")?.ok_or("a session has no directory")?,
        root: path("root")?,
        parent: id("parent")?,
        baseline: text(session, "baseline")?,
        pinned: switch(session, "pinned")?,
        changed: session.get("changed").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// A string field of at most [`MAX_TEXT`] bytes, if there.
fn text(object: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.len() <= MAX_TEXT => Ok(Some(text.clone())),
        Some(_) => Err(format!(
            "{name} is not a string of at most {MAX_TEXT} bytes"
        )),
    }
}

/// A boolean field, false if not there.
fn switch(object: &Map<String, Value>, name: &str) -> Result<bool, String> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(on)) => Ok(*on),
        Some(_) => Err(format!("{name} is not a boolean")),
    }
}

/// The most instances a state names as those its instance followed.
pub const MAX_FOLLOWS: usize = 16;

/// The instance that exported `state` and those it took over from, the newest last, at most
/// [`MAX_FOLLOWS`]; none in a state that does not say.
pub(super) fn parse_follows(state: &Map<String, Value>) -> Result<Vec<u32>, String> {
    let pid = |value: &Value| {
        value
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|&pid| pid > 1)
            .ok_or_else(|| "an instance the state follows is not a process id".to_owned())
    };
    let mut follows = match state.get("follows") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(pids)) => pids.iter().map(pid).collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("the state's follows is not a list".into()),
    };
    if let Some(from) = state.get("pid") {
        follows.push(pid(from)?);
    }
    let over = follows.len().saturating_sub(MAX_FOLLOWS);
    follows.drain(..over);
    Ok(follows)
}

/// `check-handover`'s `terminals`: whether terminals go too.
pub fn asked(params: &Map<String, Value>) -> Result<bool, String> {
    switch(params, "terminals")
}

/// `export-state`'s `terminals`: the name of the terminal library the new instance reads,
/// if terminals go too.
pub fn library(params: &Map<String, Value>) -> Result<Option<String>, String> {
    match params.get("terminals") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(terminals)) => text(terminals, "library")?
            .map(Some)
            .ok_or_else(|| "export-state's terminals need a \"library\"".to_owned()),
        Some(_) => Err("export-state's terminals is not an object".into()),
    }
}

/// `import-terminals`' `count`, or `None` for `"abandon": true`.
pub fn count(params: &Map<String, Value>) -> Result<Option<usize>, String> {
    if switch(params, "abandon")? {
        return Ok(None);
    }
    let count = size(params, "count")?;
    if count > MAX_TERMINALS {
        return Err(format!("import-terminals takes at most {MAX_TERMINALS}"));
    }
    Ok(Some(count))
}

pub use super::base64::{base64, base64_len, unbase64};

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use serde_json::json;

    use super::{SessionState, TerminalState, entry, parse};
    use crate::dev::handoff::format::Items;
    use crate::dev::handoff::tests::absolute;

    fn terminal(session: bool) -> TerminalState {
        TerminalState {
            item: 0,
            snapshot: true,
            name: "*terminal*".into(),
            rows: 24,
            columns: 80,
            pid: Some(4242),
            started: Some(SystemTime::UNIX_EPOCH + Duration::from_micros(1_700_000_000_123_456)),
            agent_reads: true,
            char_mode: false,
            session: session.then(|| SessionState {
                id: "0b1c2d3e-0000-4000-8000-000000000000".into(),
                name: "oso".into(),
                directory: absolute("/work/oso").into(),
                root: Some(absolute("/work/oso").into()),
                parent: None,
                baseline: Some("abc".into()),
                pinned: true,
                changed: 7,
            }),
        }
    }

    fn parsed(terminals: serde_json::Value) -> Result<Vec<TerminalState>, String> {
        let state = json!({ "items": [12], "terminals": terminals });
        let state = state.as_object().unwrap();
        let mut items = Items::parse(state).unwrap();
        parse(state, &mut items)
    }

    #[test]
    fn an_entry_reads_back_as_it_was_written() {
        for session in [false, true] {
            let written = terminal(session);
            let entry = entry(&written).unwrap();
            assert_eq!(parsed(json!([entry])).unwrap(), vec![written]);
        }
    }

    #[test]
    fn a_state_from_before_carries_no_terminals() {
        let state = json!({ "items": [] });
        let state = state.as_object().unwrap();
        let mut items = Items::parse(state).unwrap();
        assert!(parse(state, &mut items).unwrap().is_empty());
    }

    #[test]
    fn bad_entries_are_refused() {
        let good = entry(&terminal(true)).unwrap();
        let with = |name: &str, value: serde_json::Value| {
            let mut bad = good.clone();
            bad[name] = value;
            parsed(json!([bad]))
        };
        assert!(with("encoding", json!("png")).is_err());
        assert!(with("rows", json!(0)).is_err());
        assert!(with("columns", json!(70_000)).is_err());
        assert!(with("pid", json!(1)).is_err(), "never init");
        assert!(with("pid", json!(-3)).is_err());
        assert!(with("item", json!(1)).is_err(), "no such item");
        assert!(with("name", json!("x".repeat(5000))).is_err());
        assert!(with("agent_reads", json!("yes")).is_err());
        let mut session = good.clone();
        session["session"]["directory"] = json!("relative/dir");
        assert!(parsed(json!([session])).is_err());
        session["session"]["directory"] = json!("/abs");
        session["session"]["id"] = json!("not an id");
        assert!(parsed(json!([session])).is_err());
        assert!(
            parsed(json!([good.clone(), good])).is_err(),
            "one item each"
        );
        assert!(parsed(json!(vec![json!({}); 65])).is_err());
    }
}
