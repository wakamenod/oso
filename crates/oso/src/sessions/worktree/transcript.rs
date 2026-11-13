//! The starting session's Claude Code transcript (ADR 0121, "Worktree sessions"): where it is,
//! its last [`TAIL`] bytes, and the files and plans its edits touched. The transcript's format is
//! undocumented, so this is best effort: a line that does not parse, or parses into something
//! else, is skipped, and nothing found means the brief leaves the line out.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; [`find`] and
//! [`read_tail`] read the file system, so they run on the worker. Nothing here follows a link
//! or starts anything, and what the transcript holds is untrusted.

use std::collections::HashSet;
use std::fs::Metadata;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// The most bytes read from a transcript's end.
pub const TAIL: u64 = 1024 * 1024;

/// The most entries of `projects` looked at.
pub const MAX_PROJECT_DIRS: usize = 4096;

/// The most paths [`touched`] gives, files and plans together.
pub const MAX_TOUCHED: usize = 200;

/// The longest path [`touched`] keeps, in bytes.
const MAX_PATH: usize = 4096;

/// The longest session id [`find`] looks for.
const MAX_ID: usize = 64;

/// The tools whose input names a file they change.
const EDITS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Claude Code's configuration directory: `$CLAUDE_CONFIG_DIR` if it is absolute, else
/// `$HOME/.claude` if `HOME` is absolute (on Windows, `USERPROFILE` when `HOME` is not set).
pub fn config_dir() -> Option<PathBuf> {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if let Some(dir) = absolute("CLAUDE_CONFIG_DIR") {
        return Some(dir);
    }
    let home = absolute("HOME");
    #[cfg(windows)]
    let home = home.or_else(|| absolute("USERPROFILE"));
    home.map(|home| home.join(".claude"))
}

/// The transcript of session `id`: `<config>/projects/*/<id>.jsonl`. At most
/// [`MAX_PROJECT_DIRS`] entries of `projects` are looked at; `projects`, each directory in it
/// and the file are looked at without following a link, and the file must be a regular one
/// this user or root owns. `None` for an `id` that is not hex digits and dashes.
pub fn find(config: &Path, id: &str) -> Option<PathBuf> {
    let valid = !id.is_empty()
        && id.len() <= MAX_ID
        && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    if !valid {
        return None;
    }
    let projects = config.join("projects");
    if !is_real_dir(&projects) {
        return None;
    }
    let name = format!("{id}.jsonl");
    std::fs::read_dir(&projects)
        .ok()?
        .take(MAX_PROJECT_DIRS)
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|dir| is_real_dir(dir))
        .map(|dir| dir.join(&name))
        .find(|file| {
            std::fs::symlink_metadata(file).is_ok_and(|metadata| is_ours_and_regular(&metadata))
        })
}

/// Whether `path` is a directory, and not a link to one.
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Whether `metadata` is of a regular file this user or root owns.
fn is_ours_and_regular(metadata: &Metadata) -> bool {
    metadata.is_file() && crate::project::ours(metadata)
}

/// The last [`TAIL`] bytes of the file at `path`, and whether the file was longer, so that the
/// first line is partial and must be skipped. `None` if it is not a regular file this user or
/// root owns, or is a link.
///
/// # Errors
///
/// If the file cannot be looked at, opened or read.
pub fn read_tail(path: &Path) -> io::Result<Option<(Vec<u8>, bool)>> {
    let Some((mut file, metadata)) = oso_platform::file_meta::open_regular_here(path)? else {
        return Ok(None);
    };
    if !is_ours_and_regular(&metadata) {
        return Ok(None);
    }
    let cut = metadata.len() > TAIL;
    if cut {
        file.seek(SeekFrom::Start(metadata.len() - TAIL))?;
    }
    let mut tail = Vec::new();
    // The file may grow while it is read: never more than the tail.
    file.take(TAIL).read_to_end(&mut tail)?;
    Ok(Some((tail, cut)))
}

/// The paths a transcript's edits touched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Touched {
    /// Files changed, first seen first.
    pub files: Vec<PathBuf>,
    /// Plan files written, `.md` files under the plans directory.
    pub plans: Vec<PathBuf>,
}

/// The paths touched in JSON Lines `tail`, skipping its first line when it was `cut`: in each
/// line's `message.content`, an item of `"type": "tool_use"` whose `name` is one of the edit
/// tools and whose `input.file_path` (or `input.notebook_path`) is a string. Duplicates are
/// dropped, first seen kept, at most [`MAX_TOUCHED`]; so is a path that is empty, relative,
/// holds a control character or a `..`, or is over 4096 bytes. One that ends with `.md` under
/// `plans` (`<config>/plans`) is a plan.
pub fn touched(tail: &[u8], cut: bool, plans: &Path) -> Touched {
    let mut lines = tail.split(|&b| b == b'\n');
    if cut {
        lines.next();
    }
    let mut seen = HashSet::new();
    let mut found = Touched::default();
    for line in lines {
        if seen.len() >= MAX_TOUCHED {
            break;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        for path in edited(&value) {
            if seen.len() >= MAX_TOUCHED {
                break;
            }
            if !kept(path) || !seen.insert(path.to_owned()) {
                continue;
            }
            let path = PathBuf::from(path);
            let is_plan = path.starts_with(plans) && path.extension().is_some_and(|e| e == "md");
            if is_plan {
                found.plans.push(path);
            } else {
                found.files.push(path);
            }
        }
    }
    found
}

/// The paths named by the edit tools' calls in one transcript line.
fn edited(line: &Value) -> impl Iterator<Item = &str> {
    line.pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter(|item| {
            item.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| EDITS.contains(&name))
        })
        .filter_map(|item| {
            let input = item.get("input")?;
            ["file_path", "notebook_path"]
                .iter()
                .find_map(|key| input.get(key).and_then(Value::as_str))
        })
}

/// Whether `path` is one to keep: absolute, short enough, no control character, no `..`.
fn kept(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH
        && !path.chars().any(char::is_control)
        && Path::new(path).is_absolute()
        && !Path::new(path)
            .components()
            .any(|part| part == Component::ParentDir)
}

#[cfg(test)]
mod tests;
