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
use oso_core::review::{Filter, Saved, SavedComment, SavedFile, Shape, Side};
use serde_json::{Map, Value, json};

use model::{comment_of, comment_value, file_of, file_value, shape_of, shape_value};

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
    /// The pull request a review of one commit was stepped to from (ADR 0167), so that it is
    /// linked to that pull request's review again once its commits are listed.
    pub(crate) pull: Option<u64>,
    /// A pull request's review's: the person's last review of it (ADR 0167, step 2).
    pub(crate) checkpoint: Option<super::commits::Checkpoint>,
    /// When the person left it last, in milliseconds since the epoch, and the id of the space
    /// it was in then, as the spaces record names it (ADR 0184): which review `review-layout`
    /// continues. None in a file saved before.
    pub(crate) left: Option<u64>,
    pub(crate) space: Option<String>,
}

/// Where a window's cursor was in the review: a line of a side of a file, or the file's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) path: Box<[u8]>,
    pub(crate) line: Option<(Side, u32)>,
    /// How many rows above the cursor the window showed (ADR 0184); None in a file saved
    /// before, which puts a few back.
    pub(crate) above: Option<u32>,
}

/// What the view of a review keeps besides its model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SavedView {
    /// How the person chose it to show (step 11).
    pub(crate) choice: ReviewChoice,
    /// Whether agent cards fold.
    pub(crate) folded: bool,
    pub(crate) place: Option<Place>,
    /// The words that say which files it shows, and whether its files pane shows (step 6b).
    pub(crate) filter: Option<Filter>,
    pub(crate) files: bool,
    /// The review layout's own state as the review was left laid out (ADR 0184); None in a
    /// file saved before, or of a review not laid out.
    pub(crate) layout: Option<SavedLayout>,
}

/// A saved review read back.
#[derive(Debug)]
pub(crate) struct Decoded {
    pub(crate) header: Header,
    pub(crate) saved: Saved,
    pub(crate) view: SavedView,
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
        Scope::Since { number, from, to } => {
            format!("since\0{number}\0{}\0{}", from.as_str(), to.as_str())
        }
        Scope::PullRequest { number } => format!("pr\0{number}"),
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
    kept: &SavedView,
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
    let head = line(&header_value(header, saved.next_id));
    let view_line = line(&view_value(kept));
    // Each comment once, stopping as soon as the comments alone pass the limit.
    let mut total = head.len() + view_line.len();
    let mut comment_lines: Vec<String> = Vec::with_capacity(comments.len());
    for (comment, shape) in &comments {
        let text = line(&comment_value(comment, *shape));
        total += text.len();
        if total > max {
            return None;
        }
        comment_lines.push(text);
    }
    let shape_lines: Vec<String> = (shapes.iter().enumerate())
        .map(|(at, shape)| line(&shape_value(at, shape)))
        .collect();
    let file_lines: Vec<String> = (files.iter())
        .map(|(file, shape)| line(&file_value(file, *shape)))
        .collect();
    // What each reading takes, its files' lines with it.
    let mut taken: Vec<usize> = shape_lines.iter().map(String::len).collect();
    for ((_, shape), text) in files.iter().zip(&file_lines) {
        taken[*shape] += text.len();
    }
    total += taken.iter().sum::<usize>();
    // Past the limit, the largest readings go first, with the files on them; the comments on
    // them go outdated.
    let mut dropped: HashSet<usize> = HashSet::new();
    let mut by_size: Vec<usize> = (0..shape_lines.len()).collect();
    by_size.sort_by_key(|at| std::cmp::Reverse(shape_lines[*at].len()));
    for at in by_size {
        if total <= max {
            break;
        }
        dropped.insert(at);
        total -= taken[at];
    }
    if !dropped.is_empty() {
        for ((comment, shape), text) in comments.iter().zip(&mut comment_lines) {
            if shape.is_some_and(|shape| dropped.contains(&shape)) {
                *text = line(&comment_value(comment, None));
            }
        }
    }
    let mut out: Vec<u8> = Vec::with_capacity(total.min(max));
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(view_line.as_bytes());
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
pub(super) fn bytes_value(bytes: &[u8]) -> Value {
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
pub(super) fn value_bytes(value: &Value) -> Option<Box<[u8]>> {
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

pub(super) fn side_name(side: Side) -> &'static str {
    match side {
        Side::Old => "old",
        Side::New => "new",
    }
}

pub(super) fn side_of(value: &Value) -> Option<Side> {
    match value.as_str()? {
        "old" => Some(Side::Old),
        "new" => Some(Side::New),
        _ => None,
    }
}

fn header_value(header: &Header, next_id: u64) -> Value {
    let mut value = json!({
        "oso-review": VERSION,
        "root": bytes_value(header.root.as_os_str().as_encoded_bytes()),
        "scope": scope_value(&header.scope),
        "opened": header.opened,
        "comments": header.comments,
        "unsent": header.unsent,
        "next_id": next_id,
    });
    // Written only when there is one, so that a file of a review stepped to from no pull
    // request is as it was.
    if let (Some(pull), Some(map)) = (header.pull, value.as_object_mut()) {
        map.insert("pull".to_owned(), json!(pull));
    }
    if let (Some(checkpoint), Some(map)) = (&header.checkpoint, value.as_object_mut()) {
        let value = json!({ "base": checkpoint.base.as_str(), "head": checkpoint.head.as_str() });
        map.insert("checkpoint".to_owned(), value);
    }
    if let (Some(left), Some(map)) = (header.left, value.as_object_mut()) {
        map.insert("left".to_owned(), json!(left));
    }
    if let (Some(space), Some(map)) = (&header.space, value.as_object_mut()) {
        map.insert("space".to_owned(), json!(space));
    }
    value
}

/// Whether `text` is a space's id as the spaces record writes it: 32 lower-case hex digits.
fn is_space_id(text: &str) -> bool {
    text.len() == 32 && (text.bytes()).all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `scope` as a saved review, and a restart's state (`carry`), write it.
pub(crate) fn scope_value(scope: &Scope) -> Value {
    match scope {
        Scope::Session { baseline } => json!({ "kind": "session", "baseline": baseline.as_str() }),
        Scope::Uncommitted => json!({ "kind": "uncommitted" }),
        Scope::Staged => json!({ "kind": "staged" }),
        Scope::Unstaged => json!({ "kind": "unstaged" }),
        Scope::Branch { base } => json!({ "kind": "branch", "base": base }),
        Scope::Commit { revision } => json!({ "kind": "commit", "revision": revision }),
        Scope::Range { from, to } => json!({ "kind": "range", "from": from, "to": to }),
        Scope::PullRequest { number } => json!({ "kind": "pr", "number": number }),
        Scope::Since { number, from, to } => json!({
            "kind": "since", "number": number, "from": from.as_str(), "to": to.as_str()
        }),
    }
}

/// The scope [`scope_value`] wrote.
pub(crate) fn scope_from_value(value: &Value) -> Option<Scope> {
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
        "pr" => Scope::PullRequest {
            number: value.get("number")?.as_u64()?,
        },
        "since" => Scope::Since {
            number: value.get("number")?.as_u64()?,
            from: Id::parse(text("from")?.as_bytes())?,
            to: Id::parse(text("to")?.as_bytes())?,
        },
        _ => return None,
    })
}

fn view_value(kept: &SavedView) -> Value {
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
        if let Some(above) = place.above {
            value.insert("above".into(), above.into());
        }
        Value::Object(value)
    });
    let filter = kept.filter.as_ref().map(Filter::text);
    let mut value = json!({
        "choice": choice,
        "folded": kept.folded,
        "place": place,
        "filter": filter,
        "files": kept.files,
    });
    // Written only when there is one, so that a view of a review not laid out is as it was.
    if let (Some(layout), Some(map)) = (&kept.layout, value.as_object_mut()) {
        map.insert("layout".to_owned(), layout::layout_value(layout));
    }
    json!({ "kept": value })
}

fn view_of(value: &Value) -> Option<SavedView> {
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
            // Rows above the cursor, in a file saved since ADR 0184.
            let above = (place.get("above").and_then(Value::as_u64))
                .and_then(|above| u32::try_from(above).ok());
            Some(Place { path, line, above })
        }
    };
    // Neither is in a review saved before step 6b; a filter read is cut as one typed is.
    let filter = match value.get("filter") {
        None | Some(Value::Null) => None,
        Some(filter) => Filter::new(filter.as_str()?),
    };
    let files = match value.get("files") {
        None => false,
        Some(files) => files.as_bool()?,
    };
    // One not read is left out alone: the rest of the view stands (ADR 0184).
    let layout = value.get("layout").and_then(layout::layout_of);
    Some(SavedView {
        choice,
        folded: value.get("folded")?.as_bool()?,
        place,
        filter,
        files,
        layout,
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
        scope: scope_from_value(value.get("scope")?)?,
        opened: value.get("opened")?.as_u64()?,
        comments: count("comments")?,
        unsent: count("unsent")?,
        // A number of digits a pull request may have, or none (ADR 0167).
        pull: value
            .get("pull")
            .and_then(Value::as_u64)
            .filter(|number| *number > 0 && number.to_string().len() <= 18),
        // Two whole ids, or none (ADR 0167, step 2).
        checkpoint: value.get("checkpoint").and_then(|checkpoint| {
            let id = |name: &str| Id::parse(checkpoint.get(name)?.as_str()?.as_bytes());
            Some(super::commits::Checkpoint {
                base: id("base")?,
                head: id("head")?,
            })
        }),
        // When and where the person left it (ADR 0184), in a file saved since.
        left: value.get("left").and_then(Value::as_u64),
        space: (value.get("space").and_then(Value::as_str))
            .filter(|space| is_space_id(space))
            .map(str::to_owned),
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
pub(crate) fn decode(bytes: &[u8]) -> Result<Decoded, &'static str> {
    let mut lines = bytes.split(|byte| *byte == b'\n');
    let (header, next_id) = (lines.next())
        .and_then(header_of)
        .ok_or("not a saved review of this version")?;
    let mut view = SavedView::default();
    let mut shapes: HashMap<u64, Arc<Shape>> = HashMap::new();
    let mut files = Vec::new();
    let mut current: HashMap<Box<[u8]>, Arc<Shape>> = HashMap::new();
    let mut comments: Vec<SavedComment> = Vec::new();
    let mut ids: HashSet<u64> = HashSet::new();
    let mut left_out = 0;
    for line in lines.filter(|line| !line.is_empty()) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            left_out += 1;
            continue;
        };
        let taken = if let Some(found) = value.get("kept") {
            view_of(found).map(|found| view = found).is_some()
        } else if let Some(at) = value.get("shape").filter(|_| value.get("base").is_some()) {
            match (at.as_u64(), shape_of(&value)) {
                (Some(at), Some(shape)) => shapes.insert(at, Arc::new(shape)).is_none(),
                _ => false,
            }
        } else if value.get("file").is_some() {
            file_of(&value, &shapes)
                .map(|file| {
                    current.insert(file.path.clone(), Arc::clone(&file.shape));
                    files.push(file);
                })
                .is_some()
        } else if value.get("comment").is_some() {
            // A reply to a comment left out goes with it, and one placed on another reading
            // than its file's current one, as a save never writes it, is outdated.
            comment_of(&value, &shapes)
                .map(|mut comment| {
                    let on_current = (comment.shape.as_ref())
                        .zip(current.get(&*comment.path))
                        .is_some_and(|(shape, current)| Arc::ptr_eq(shape, current));
                    comment.outdated |= !on_current;
                    comment
                })
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
    Ok(Decoded {
        header,
        saved: Saved {
            files,
            comments,
            next_id,
        },
        view,
        left_out,
    })
}

mod layout;
mod model;

pub(crate) use layout::{SavedLayout, SlotShows};

#[cfg(test)]
#[path = "saved_tests.rs"]
mod tests;
