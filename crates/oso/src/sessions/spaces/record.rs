//! The spaces oso keeps (ADR 0136): `oso/spaces` in the per-user state directory, beside
//! `sessions`, one JSON object a line. Reading it starts nothing.
//!
//! Must not depend on: threads, the UI or `app`; the sessions worker reads and writes it,
//! never the UI thread.
//!
//! - A space's description is written as `description` and, cut to what a build before ADR
//!   0159 reads, as `title`, which such a build keeps; its display name as `display`, which
//!   such a build leaves out.
//! - It is read as `sessions` is: only if the directory is private and the file a regular one
//!   of this user's with one name, at most [`MAX_BYTES`] and [`MAX_SPACES`] spaces. A line that
//!   does not hold a whole, valid space is left out, and only how many were is logged.
//! - A write replaces the file whole, through a `.part`, or leaves it as it was. It keeps the
//!   spaces this instance never knew, so that two instances do not drop each other's.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use oso_platform::privacy::PrivateDir;
use serde_json::{Map, Value, json};

use super::stored::{
    MAX_DESCRIPTION, MAX_DISPLAY, MAX_EARLIER, MAX_ISSUE, MAX_MEMBERS, MAX_OLD_TITLE,
    MAX_WORKTREES, Member, SpaceId, Stored, clean_display, cut_bytes, slug,
};
use crate::sessions::identity::SessionId;
use crate::sessions::record::{read_file, replace_file};

/// The file's name in its directory.
const FILE: &str = "spaces";
/// Where a write goes first, so that the file is whole or not there.
const PART: &str = "spaces.part";
/// The most bytes read of the file; a larger one is not read. Room for as many spaces as it
/// keeps, each with a whole description (ADR 0159).
pub const MAX_BYTES: u64 = 512 * 1024;
/// The most spaces read or written.
pub const MAX_SPACES: usize = 256;
/// The longest display name read, in bytes, before it is cleaned to at most
/// [`MAX_DISPLAY`].
const MAX_DISPLAY_LINE: usize = 4 * MAX_DISPLAY;
/// The longest path kept, in bytes.
const MAX_PATH: usize = 4096;

/// What the `spaces` file gave.
#[derive(Debug, PartialEq, Eq)]
pub enum Kept {
    /// There is none, so the spaces are made from the sessions, once.
    Missing,
    /// What it held.
    Read(Vec<Stored>),
    /// It may not be read, which is logged: it is not replaced, and the sessions' spaces stay
    /// as their record names them.
    Refused,
}

/// What the file in `state_dir` holds.
pub fn read(state_dir: &Path) -> Kept {
    match read_checked(state_dir) {
        Ok(Some(spaces)) => Kept::Read(spaces),
        Ok(None) => Kept::Missing,
        Err(e) => {
            log::warn!("not reading the spaces: {e}");
            Kept::Refused
        }
    }
}

/// What the file holds: `None` if there is none, and an error if there is one that may not be
/// read, which a write must then not replace.
fn read_checked(state_dir: &Path) -> io::Result<Option<Vec<Stored>>> {
    Ok(read_file(state_dir, FILE, MAX_BYTES)?.map(|bytes| parse(&bytes)))
}

/// The spaces in the file's bytes, the last of an id kept, at most [`MAX_SPACES`].
fn parse(bytes: &[u8]) -> Vec<Stored> {
    let mut spaces: Vec<Stored> = Vec::new();
    // Each id's place, so that a file of many lines costs one look a line.
    let mut places: std::collections::HashMap<SpaceId, usize> = std::collections::HashMap::new();
    let mut bad = 0;
    for line in bytes.split(|&b| b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Some(space) = parse_line(line) else {
            bad += 1;
            continue;
        };
        match places.get(&space.id) {
            Some(&at) => spaces[at] = space,
            None => {
                places.insert(space.id.clone(), spaces.len());
                spaces.push(space);
            }
        }
    }
    if bad > 0 {
        log::warn!("left out {bad} lines of the spaces that hold no space");
    }
    spaces.truncate(MAX_SPACES);
    spaces
}

/// One line's space, if it is whole and valid.
fn parse_line(line: &[u8]) -> Option<Stored> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let object = value.as_object()?;
    let text = |key: &str, most: usize| match object.get(key) {
        None => Some(String::new()),
        Some(Value::String(text)) if text.len() <= most && all_plain(text) => Some(text.clone()),
        Some(_) => None,
    };
    let name = match object.get("name") {
        Some(Value::String(name)) if slug(name) == *name => name.clone(),
        _ => return None,
    };
    let list = |key: &str, most: usize| match object.get(key) {
        None => Some(Vec::new()),
        Some(Value::Array(items)) if items.len() <= most => Some(items.as_slice().to_vec()),
        Some(_) => None,
    };
    let members = list("members", MAX_MEMBERS)?
        .iter()
        .map(member)
        .collect::<Option<Vec<_>>>()?;
    let worktrees = list("worktrees", MAX_WORKTREES)?
        .iter()
        .map(|item| path(item.as_str()?))
        .collect::<Option<Vec<_>>>()?;
    let earlier = list("earlier", MAX_EARLIER)?
        .iter()
        .map(|item| SessionId::parse(item.as_str()?))
        .collect::<Option<Vec<_>>>()?;
    let orchestrator = match object.get("orchestrator") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(SessionId::parse(id)?),
        Some(_) => return None,
    };
    let directory = match object.get("directory") {
        None | Some(Value::Null) => None,
        Some(Value::String(dir)) => Some(path(dir)?),
        Some(_) => return None,
    };
    Some(Stored {
        id: SpaceId::parse(object.get("id")?.as_str()?)?,
        name,
        // Cleaned again rather than refused: what is drawn may change with oso's tables, and a
        // space must not be lost for its name (ADR 0159).
        display: match object.get("display") {
            None => String::new(),
            Some(Value::String(display)) if display.len() <= MAX_DISPLAY_LINE => {
                clean_display(display)
            }
            Some(_) => return None,
        },
        description: match object.get("description") {
            Some(_) => text("description", MAX_DESCRIPTION)?,
            None => text("title", MAX_DESCRIPTION)?,
        },
        issue: text("issue", MAX_ISSUE)?,
        members,
        directory,
        directory_identity: identity(object.get("directory_identity"))?,
        archived: match object.get("archived") {
            None => false,
            Some(value) => value.as_bool()?,
        },
        orchestrator,
        earlier,
        worktrees,
        created: object.get("created")?.as_u64()?,
        changed: object.get("changed")?.as_u64()?,
    })
}

/// A member's object: its root, and its identity if it has one.
pub fn member(value: &Value) -> Option<Member> {
    let object = value.as_object()?;
    Some(Member {
        root: path(object.get("root")?.as_str()?)?,
        identity: identity(object.get("identity"))?,
    })
}

/// A member's object, as [`member`] reads it; `None` for a root that is not UTF-8.
pub fn member_value(member: &Member) -> Option<Value> {
    let mut object = Map::new();
    object.insert("root".into(), json!(member.root.to_str()?));
    if let Some((dev, ino)) = member.identity {
        object.insert("identity".into(), json!([dev, ino]));
    }
    Some(Value::Object(object))
}

/// An identity, `[device, inode]`, or none: `Some(None)` when it is left out.
fn identity(value: Option<&Value>) -> Option<Option<(u64, u64)>> {
    match value {
        None | Some(Value::Null) => Some(None),
        Some(Value::Array(pair)) if pair.len() == 2 => {
            Some(Some((pair[0].as_u64()?, pair[1].as_u64()?)))
        }
        Some(_) => None,
    }
}

/// An absolute path of at most [`MAX_PATH`] bytes.
fn path(text: &str) -> Option<PathBuf> {
    let path = PathBuf::from(text);
    (text.len() <= MAX_PATH && path.is_absolute() && !text.contains('\0')).then_some(path)
}

/// Whether `text` has no control or invisible character, so that a description or an issue never
/// moves a row or reads otherwise than it is (ADR 0136, step 1b, P5).
fn all_plain(text: &str) -> bool {
    text.chars().all(super::stored::is_plain)
}

/// One space as its line. A path that is not UTF-8 cannot be written, and its space is not
/// kept.
fn line(space: &Stored) -> Option<String> {
    let paths = |paths: &[PathBuf]| -> Option<Vec<Value>> {
        paths.iter().map(|p| Some(json!(p.to_str()?))).collect()
    };
    let pair = |pair: Option<(u64, u64)>| pair.map(|(dev, ino)| json!([dev, ino]));
    let members = (space.members.iter())
        .map(member_value)
        .collect::<Option<Vec<_>>>()?;
    let mut object = Map::new();
    object.insert("id".into(), json!(space.id.as_str()));
    object.insert("name".into(), json!(space.name));
    if !space.display.is_empty() {
        object.insert("display".into(), json!(space.display));
    }
    object.insert("description".into(), json!(space.description));
    let title = cut_bytes(&space.description, MAX_OLD_TITLE).trim_end();
    object.insert("title".into(), json!(title));
    object.insert("issue".into(), json!(space.issue));
    object.insert("members".into(), Value::Array(members));
    if let Some(directory) = &space.directory {
        object.insert("directory".into(), json!(directory.to_str()?));
    }
    if let Some(identity) = pair(space.directory_identity) {
        object.insert("directory_identity".into(), identity);
    }
    object.insert("archived".into(), json!(space.archived));
    if let Some(orchestrator) = &space.orchestrator {
        object.insert("orchestrator".into(), json!(orchestrator.as_str()));
    }
    let earlier: Vec<Value> = space.earlier.iter().map(|id| json!(id.as_str())).collect();
    object.insert("earlier".into(), Value::Array(earlier));
    object.insert("worktrees".into(), Value::Array(paths(&space.worktrees)?));
    object.insert("created".into(), json!(space.created));
    object.insert("changed".into(), json!(space.changed));
    Some(Value::Object(object).to_string())
}

/// Writes `spaces` in `state_dir`, which is made private if it is missing, with the spaces
/// already there whose ids are not in `known`: another instance's. Of a space both have, the
/// one changed last is kept, so that a stale instance does not bring back what another took
/// out. At most [`MAX_SPACES`], ours first, and [`MAX_BYTES`] in all, a space too large to fit
/// left out, so that the file can always be read back. A file there that may not be read is
/// not replaced.
pub fn write(state_dir: &Path, spaces: &[Stored], known: &HashSet<SpaceId>) -> io::Result<()> {
    let theirs = read_checked(state_dir)?.unwrap_or_default();
    let directory = PrivateDir::open(state_dir)?;
    let newer = |ours: &Stored| {
        theirs
            .iter()
            .find(|space| space.id == ours.id && space.changed > ours.changed)
            .unwrap_or(ours)
            .clone()
    };
    let kept = theirs
        .iter()
        .filter(|space| !known.contains(&space.id))
        .filter(|space| spaces.iter().all(|ours| ours.id != space.id))
        .cloned();
    let all: Vec<Stored> = spaces.iter().map(newer).chain(kept).collect();
    let mut text = String::new();
    let most = usize::try_from(MAX_BYTES).unwrap_or(usize::MAX);
    for line in all.iter().filter_map(line).take(MAX_SPACES) {
        if text.len() + line.len() + 1 > most {
            continue;
        }
        text.push_str(&line);
        text.push('\n');
    }
    replace_file(&directory, FILE, PART, &text)
}

#[cfg(all(test, unix))]
mod tests;
