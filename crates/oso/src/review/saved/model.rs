//! A saved review's lines of the model (step 6): each reading, each file's marks and each
//! comment, as [`super::encode`] writes them and [`super::decode`] reads them back.
//!
//! Must not depend on: `app`, threads, the editor or the file system.

use std::collections::HashMap;
use std::sync::Arc;

use oso_core::review::{
    Anchor, Author, Base, Decision, Hunk, MAX_COMMENT_BYTES, SavedComment, SavedFile, Shape,
};
use serde_json::{Value, json};

use super::{bytes_value, side_name, side_of, value_bytes};

pub(super) fn shape_value(at: usize, shape: &Shape) -> Value {
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

pub(super) fn shape_of(value: &Value) -> Option<Shape> {
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

pub(super) fn file_value(file: &SavedFile, shape: usize) -> Value {
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

pub(super) fn file_of(value: &Value, shapes: &HashMap<u64, Arc<Shape>>) -> Option<SavedFile> {
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

pub(super) fn comment_value(comment: &SavedComment, shape: Option<usize>) -> Value {
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

pub(super) fn comment_of(value: &Value, shapes: &HashMap<u64, Arc<Shape>>) -> Option<SavedComment> {
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
