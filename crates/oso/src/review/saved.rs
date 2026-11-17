//! A review as it is written to disk (ADR 0122, "Persistence", step 6): one file per project
//! and scope in `reviews/` under the state directory, JSON Lines through `serde_json`.
//!
//! Must not depend on: `app`, threads, the editor or the file system; the store reads and
//! writes the bytes this makes and takes, on a thread of its own.
//!
//! - The first line is the header: the format's version, the project's root, the scope with its
//!   revisions as given and the session's baseline, when it was last opened, and the counts a
//!   list of saved reviews shows. Then what the view keeps (`kept`), each reading once
//!   (`shape`), each file's marks (`file`) and each comment (`comment`), oldest first.
//! - Paths and lines are bytes: a JSON string when they are UTF-8, else `{"hex": …}`.
//! - Nothing of a file that may hold secrets is written but its comments' places and texts:
//!   no reading of it, so its comments come back outdated (Q-F).
//! - A review is at most [`MAX_BYTES`]; past it, the largest readings are left out first and
//!   their comments come back outdated (Q-E). One whose comments alone pass it is not written.
//! - Read back, a line that is not one this writes is left out, with a comment replying to one
//!   left out; [`oso_core::review::Review::restore`] then checks the rest whole.
//! - Nothing here logs a file's text or a comment's.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::editor::ReviewChoice;
use oso_core::review::{
    Anchor, Author, Base, Decision, Hunk, MAX_COMMENT_BYTES, Saved, SavedComment, SavedFile, Shape,
    Side,
};
use serde_json::{Map, Value, json};

use super::Scope;
use crate::git::Id;

/// The format's version: a file of another is not read.
pub(crate) const VERSION: u64 = 1;
/// The most bytes a saved review takes.
pub(crate) const MAX_BYTES: usize = 16 * 1024 * 1024;
/// What a saved review's file name ends with.
pub(crate) const SUFFIX: &str = ".review";

/// What a saved review's first line says of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    /// The project's root, as the review found it.
    pub(crate) root: PathBuf,
    pub(crate) scope: Scope,
    /// When it was last opened, in milliseconds since the epoch.
    pub(crate) opened: u64,
    /// How many comments it holds, and of them the person's not sent.
    pub(crate) comments: usize,
    pub(crate) unsent: usize,
}

/// Where a window's cursor was in the review: a line of a side of a file, or the file's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) path: Box<[u8]>,
    pub(crate) line: Option<(Side, u32)>,
}

/// What the view of a review keeps besides its model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Kept {
    /// How the person chose it to show (step 11).
    pub(crate) choice: ReviewChoice,
    /// Whether agent cards fold.
    pub(crate) folded: bool,
    pub(crate) place: Option<Place>,
}

/// A saved review read back.
#[derive(Debug)]
pub(crate) struct Read {
    pub(crate) header: Header,
    pub(crate) saved: Saved,
    pub(crate) kept: Kept,
    /// How many lines were left out as not ones this writes.
    pub(crate) left_out: usize,
}

/// The name of the file the review of `scope` in the project at `root` is saved in: the same
/// for every session baseline, and a branch, commit or range by the revisions given.
pub(crate) fn file_name(root: &Path, scope: &Scope) -> String {
    let mut key = root.as_os_str().as_encoded_bytes().to_vec();
    key.push(0);
    key.extend_from_slice(scope_key(scope).as_bytes());
    let mut name = String::with_capacity(32 + SUFFIX.len());
    for byte in fnv128(&key).to_be_bytes() {
        let _ = write!(name, "{byte:02x}");
    }
    name.push_str(SUFFIX);
    name
}

/// What tells scopes apart in a file's name: the session scope whatever its baseline.
fn scope_key(scope: &Scope) -> String {
    match scope {
        Scope::Session { .. } => "session".into(),
        Scope::Uncommitted => "uncommitted".into(),
        Scope::Staged => "staged".into(),
        Scope::Unstaged => "unstaged".into(),
        Scope::Branch { base } => format!("branch\0{base}"),
        Scope::Commit { revision } => format!("commit\0{revision}"),
        Scope::Range { from, to } => format!("range\0{from}\0{to}"),
    }
}

/// FNV-1a over 128 bits: names files apart, guarding nothing, as the header says what each
/// is and is checked as it is read.
fn fnv128(bytes: &[u8]) -> u128 {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u128::from(*byte)).wrapping_mul(PRIME)
    })
}

/// `saved` as a file, with `header` and `kept`, or `None` when its comments alone pass
/// `max` bytes. Nothing of a file `withheld` names is written but its comments' places and
/// texts, and past `max` the largest readings are left out first.
pub(crate) fn encode(
    header: &Header,
    saved: &Saved,
    kept: &Kept,
    withheld: &dyn Fn(&[u8]) -> bool,
    max: usize,
) -> Option<Vec<u8>> {
    // Each reading once, by its place in memory.
    let mut shapes = Readings::default();
    let files: Vec<(&SavedFile, usize)> = (saved.files.iter())
        .filter(|file| !withheld(&file.path))
        .map(|file| (file, shapes.number(&file.shape)))
        .collect();
    let comments: Vec<(&SavedComment, Option<usize>)> = (saved.comments.iter())
        .map(|comment| {
            let shape = (comment.shape.as_ref())
                .filter(|_| !withheld(&comment.path))
                .map(|shape| shapes.number(shape));
            (comment, shape)
        })
        .collect();
    let shapes = shapes.list;
    let shape_lines: Vec<String> = (shapes.iter().enumerate())
        .map(|(at, shape)| line(&shape_value(at, shape)))
        .collect();
    let head = line(&header_value(header, saved.next_id));
    let kept_line = line(&kept_value(kept));
    let file_lines: Vec<String> = (files.iter())
        .map(|(file, shape)| line(&file_value(file, *shape)))
        .collect();
    let comment_lines = |dropped: &HashSet<usize>| -> Vec<String> {
        (comments.iter())
            .map(|(comment, shape)| {
                let shape = shape.filter(|shape| !dropped.contains(shape));
                line(&comment_value(comment, shape))
            })
            .collect()
    };
    // Past the limit, the largest readings go first, with the files and comments on them.
    let mut dropped: HashSet<usize> = HashSet::new();
    let mut by_size: Vec<usize> = (0..shape_lines.len()).collect();
    by_size.sort_by_key(|at| std::cmp::Reverse(shape_lines[*at].len()));
    let fixed = head.len()
        + kept_line.len()
        + comment_lines(&dropped)
            .iter()
            .map(String::len)
            .sum::<usize>();
    let mut total = fixed
        + shape_lines.iter().map(String::len).sum::<usize>()
        + file_lines.iter().map(String::len).sum::<usize>();
    for at in by_size {
        if total <= max {
            break;
        }
        dropped.insert(at);
        total -= shape_lines[at].len();
        for ((_, shape), text) in files.iter().zip(&file_lines) {
            if *shape == at {
                total -= text.len();
            }
        }
    }
    let comment_lines = comment_lines(&dropped);
    let mut out: Vec<u8> = Vec::with_capacity(total.min(max));
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(kept_line.as_bytes());
    for (at, text) in shape_lines.iter().enumerate() {
        if !dropped.contains(&at) {
            out.extend_from_slice(text.as_bytes());
        }
    }
    for ((_, shape), text) in files.iter().zip(&file_lines) {
        if !dropped.contains(shape) {
            out.extend_from_slice(text.as_bytes());
        }
    }
    for text in &comment_lines {
        out.extend_from_slice(text.as_bytes());
    }
    (out.len() <= max).then_some(out)
}

/// The readings a saved review writes, each once, numbered in the order first met.
#[derive(Default)]
struct Readings<'a> {
    list: Vec<&'a Arc<Shape>>,
    index: HashMap<*const Shape, usize>,
}

impl<'a> Readings<'a> {
    fn number(&mut self, shape: &'a Arc<Shape>) -> usize {
        let list = &mut self.list;
        *self.index.entry(Arc::as_ptr(shape)).or_insert_with(|| {
            list.push(shape);
            list.len() - 1
        })
    }
}

/// `value` as one line.
fn line(value: &Value) -> String {
    let mut text = value.to_string();
    text.push('\n');
    text
}

/// What `bytes` are written as: a string when they are UTF-8, else their hex digits.
fn bytes_value(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => Value::String(text.to_owned()),
        Err(_) => {
            let mut hex = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                let _ = write!(hex, "{byte:02x}");
            }
            json!({ "hex": hex })
        }
    }
}

/// The bytes [`bytes_value`] wrote.
fn value_bytes(value: &Value) -> Option<Box<[u8]>> {
    if let Some(text) = value.as_str() {
        return Some(text.as_bytes().into());
    }
    let hex = value.get("hex")?.as_str()?.as_bytes();
    if hex.len() % 2 != 0 {
        return None;
    }
    let digit = |byte: u8| char::from(byte).to_digit(16);
    hex.chunks_exact(2)
        .map(|pair| u8::try_from(digit(pair[0])? * 16 + digit(pair[1])?).ok())
        .collect()
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Old => "old",
        Side::New => "new",
    }
}

fn side_of(value: &Value) -> Option<Side> {
    match value.as_str()? {
        "old" => Some(Side::Old),
        "new" => Some(Side::New),
        _ => None,
    }
}

fn header_value(header: &Header, next_id: u64) -> Value {
    json!({
        "oso-review": VERSION,
        "root": bytes_value(header.root.as_os_str().as_encoded_bytes()),
        "scope": scope_value(&header.scope),
        "opened": header.opened,
        "comments": header.comments,
        "unsent": header.unsent,
        "next_id": next_id,
    })
}

fn scope_value(scope: &Scope) -> Value {
    match scope {
        Scope::Session { baseline } => json!({ "kind": "session", "baseline": baseline.as_str() }),
        Scope::Uncommitted => json!({ "kind": "uncommitted" }),
        Scope::Staged => json!({ "kind": "staged" }),
        Scope::Unstaged => json!({ "kind": "unstaged" }),
        Scope::Branch { base } => json!({ "kind": "branch", "base": base }),
        Scope::Commit { revision } => json!({ "kind": "commit", "revision": revision }),
        Scope::Range { from, to } => json!({ "kind": "range", "from": from, "to": to }),
    }
}

fn scope_of(value: &Value) -> Option<Scope> {
    let text = |name: &str| value.get(name)?.as_str().map(str::to_owned);
    Some(match value.get("kind")?.as_str()? {
        "session" => Scope::Session {
            baseline: Id::parse(text("baseline")?.as_bytes())?,
        },
        "uncommitted" => Scope::Uncommitted,
        "staged" => Scope::Staged,
        "unstaged" => Scope::Unstaged,
        "branch" => Scope::Branch {
            base: text("base")?,
        },
        "commit" => Scope::Commit {
            revision: text("revision")?,
        },
        "range" => Scope::Range {
            from: text("from")?,
            to: text("to")?,
        },
        _ => return None,
    })
}

fn kept_value(kept: &Kept) -> Value {
    let choice = match kept.choice {
        ReviewChoice::Auto => "auto",
        ReviewChoice::Unified => "unified",
        ReviewChoice::Panes => "panes",
    };
    let place = kept.place.as_ref().map(|place| {
        let mut value = Map::new();
        value.insert("path".into(), bytes_value(&place.path));
        if let Some((side, line)) = place.line {
            value.insert("side".into(), side_name(side).into());
            value.insert("line".into(), line.into());
        }
        Value::Object(value)
    });
    json!({ "kept": { "choice": choice, "folded": kept.folded, "place": place } })
}

fn kept_of(value: &Value) -> Option<Kept> {
    let choice = match value.get("choice")?.as_str()? {
        "auto" => ReviewChoice::Auto,
        "unified" => ReviewChoice::Unified,
        "panes" => ReviewChoice::Panes,
        _ => return None,
    };
    let place = match value.get("place") {
        None | Some(Value::Null) => None,
        Some(place) => {
            let path = value_bytes(place.get("path")?)?;
            let line = match place.get("side") {
                None => None,
                Some(side) => Some((
                    side_of(side)?,
                    u32::try_from(place.get("line")?.as_u64()?).ok()?,
                )),
            };
            Some(Place { path, line })
        }
    };
    Some(Kept {
        choice,
        folded: value.get("folded")?.as_bool()?,
        place,
    })
}

fn shape_value(at: usize, shape: &Shape) -> Value {
    let base = match shape.base() {
        Base::Empty => Value::from("empty"),
        Base::Unknown => Value::from("unknown"),
        Base::Blob(id) => json!({ "blob": bytes_value(id) }),
    };
    let lines = |lines: &[Box<[u8]>]| -> Value {
        Value::Array(lines.iter().map(|line| bytes_value(line)).collect())
    };
    let hunks: Vec<Value> = (shape.hunks().iter())
        .map(|hunk| {
            json!({
                "old": hunk.old_start,
                "new": hunk.new_start,
                "removed": lines(&hunk.removed),
                "added": lines(&hunk.added),
            })
        })
        .collect();
    json!({ "shape": at, "base": base, "hunks": hunks })
}

fn shape_of(value: &Value) -> Option<Shape> {
    let base = match value.get("base")? {
        Value::String(base) if base == "empty" => Base::Empty,
        Value::String(base) if base == "unknown" => Base::Unknown,
        base => Base::Blob(value_bytes(base.get("blob")?)?),
    };
    let lines = |value: &Value| -> Option<Vec<Box<[u8]>>> {
        value.as_array()?.iter().map(value_bytes).collect()
    };
    let hunks = (value.get("hunks")?.as_array()?.iter())
        .map(|hunk| {
            Some(Hunk {
                old_start: u32::try_from(hunk.get("old")?.as_u64()?).ok()?,
                new_start: u32::try_from(hunk.get("new")?.as_u64()?).ok()?,
                removed: lines(hunk.get("removed")?)?,
                added: lines(hunk.get("added")?)?,
            })
        })
        .collect::<Option<Vec<Hunk>>>()?;
    Shape::new(base, hunks).ok()
}

fn file_value(file: &SavedFile, shape: usize) -> Value {
    let marks: Vec<&str> = (file.marks.iter())
        .map(|mark| match mark {
            (Some(Decision::Accepted), true) => "av",
            (Some(Decision::Accepted), false) => "a",
            (Some(Decision::Rejected), true) => "rv",
            (Some(Decision::Rejected), false) => "r",
            (None, true) => "v",
            (None, false) => "",
        })
        .collect();
    json!({
        "file": bytes_value(&file.path),
        "shape": shape,
        "marks": marks,
        "dropped": file.dropped,
    })
}

fn file_of(value: &Value, shapes: &HashMap<u64, Arc<Shape>>) -> Option<SavedFile> {
    let shape = shapes.get(&value.get("shape")?.as_u64()?)?;
    let marks = (value.get("marks")?.as_array()?.iter())
        .map(|mark| {
            Some(match mark.as_str()? {
                "av" => (Some(Decision::Accepted), true),
                "a" => (Some(Decision::Accepted), false),
                "rv" => (Some(Decision::Rejected), true),
                "r" => (Some(Decision::Rejected), false),
                "v" => (None, true),
                "" => (None, false),
                _ => return None,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(SavedFile {
        path: value_bytes(value.get("file")?)?,
        shape: Arc::clone(shape),
        marks,
        dropped: usize::try_from(value.get("dropped")?.as_u64()?).ok()?,
    })
}

fn comment_value(comment: &SavedComment, shape: Option<usize>) -> Value {
    let anchor = match comment.anchor {
        Anchor::Line { side, number } => json!({ "side": side_name(side), "line": number }),
        Anchor::Hunk { index } => json!({ "hunk": index }),
    };
    json!({
        "comment": comment.id,
        "author": match comment.author {
            Author::Person => "person",
            Author::Agent => "agent",
        },
        "path": bytes_value(&comment.path),
        "anchor": anchor,
        "shape": shape,
        "text": &*comment.text,
        "reply_to": comment.reply_to,
        "sent": comment.sent,
        "resolved": comment.resolved,
        // A comment whose reading is not written is outdated for good.
        "outdated": comment.outdated || shape.is_none(),
        "read": comment.read,
    })
}

fn comment_of(value: &Value, shapes: &HashMap<u64, Arc<Shape>>) -> Option<SavedComment> {
    let anchor = value.get("anchor")?;
    let anchor = match anchor.get("hunk") {
        Some(index) => Anchor::Hunk {
            index: usize::try_from(index.as_u64()?).ok()?,
        },
        None => Anchor::Line {
            side: side_of(anchor.get("side")?)?,
            number: u32::try_from(anchor.get("line")?.as_u64()?).ok()?,
        },
    };
    let author = match value.get("author")?.as_str()? {
        "person" => Author::Person,
        "agent" => Author::Agent,
        _ => return None,
    };
    let text = value.get("text")?.as_str()?;
    if text.len() > MAX_COMMENT_BYTES {
        return None;
    }
    // A reading not read back leaves the comment outdated for good.
    let shape = match value.get("shape")? {
        Value::Null => None,
        at => shapes.get(&at.as_u64()?).cloned(),
    };
    let flag = |name: &str| value.get(name)?.as_bool();
    Some(SavedComment {
        id: value.get("comment")?.as_u64()?,
        author,
        path: Arc::from(value_bytes(value.get("path")?)?),
        anchor,
        outdated: flag("outdated")? || shape.is_none(),
        shape,
        text: text.into(),
        reply_to: match value.get("reply_to")? {
            Value::Null => None,
            id => Some(id.as_u64()?),
        },
        sent: flag("sent")?,
        resolved: flag("resolved")?,
        read: flag("read")?,
    })
}

/// The header `line`, the first of a saved review, says, and the next comment's id.
fn header_of(line: &[u8]) -> Option<(Header, u64)> {
    let value: Value = serde_json::from_slice(line).ok()?;
    if value.get("oso-review")?.as_u64()? != VERSION {
        return None;
    }
    let root = value_bytes(value.get("root")?)?;
    let root = PathBuf::from(std::str::from_utf8(&root).ok()?);
    let count = |name: &str| usize::try_from(value.get(name)?.as_u64()?).ok();
    let header = Header {
        root,
        scope: scope_of(value.get("scope")?)?,
        opened: value.get("opened")?.as_u64()?,
        comments: count("comments")?,
        unsent: count("unsent")?,
    };
    Some((header, value.get("next_id")?.as_u64()?))
}

/// What the first line of `bytes`, a saved review's start, says, if it is a header this
/// writes.
pub(crate) fn header(bytes: &[u8]) -> Option<Header> {
    let first = bytes.split(|byte| *byte == b'\n').next()?;
    Some(header_of(first)?.0)
}

/// The saved review `bytes` hold, or why it is not one: no header of this version. Lines that
/// are not ones this writes are left out, and counted.
pub(crate) fn decode(bytes: &[u8]) -> Result<Read, &'static str> {
    let mut lines = bytes.split(|byte| *byte == b'\n');
    let (header, next_id) = (lines.next())
        .and_then(header_of)
        .ok_or("not a saved review of this version")?;
    let mut kept = Kept::default();
    let mut shapes: HashMap<u64, Arc<Shape>> = HashMap::new();
    let mut files = Vec::new();
    let mut comments: Vec<SavedComment> = Vec::new();
    let mut ids: HashSet<u64> = HashSet::new();
    let mut left_out = 0;
    for line in lines.filter(|line| !line.is_empty()) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            left_out += 1;
            continue;
        };
        let taken = if let Some(found) = value.get("kept") {
            kept_of(found).map(|found| kept = found).is_some()
        } else if let Some(at) = value.get("shape").filter(|_| value.get("base").is_some()) {
            match (at.as_u64(), shape_of(&value)) {
                (Some(at), Some(shape)) => shapes.insert(at, Arc::new(shape)).is_none(),
                _ => false,
            }
        } else if value.get("file").is_some() {
            file_of(&value, &shapes)
                .map(|file| files.push(file))
                .is_some()
        } else if value.get("comment").is_some() {
            // A reply to a comment left out goes with it.
            comment_of(&value, &shapes)
                .filter(|comment| comment.reply_to.is_none_or(|to| ids.contains(&to)))
                .map(|comment| {
                    ids.insert(comment.id);
                    comments.push(comment);
                })
                .is_some()
        } else {
            false
        };
        left_out += usize::from(!taken);
    }
    Ok(Read {
        header,
        saved: Saved {
            files,
            comments,
            next_id,
        },
        kept,
        left_out,
    })
}

#[cfg(test)]
#[path = "saved_tests.rs"]
mod tests;
