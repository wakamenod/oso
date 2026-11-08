//! The state's JSON (ADR 0047, ADR 0090): what is written of a carried buffer's file and of
//! a window's selections, and reading a whole state back, checked before anything changes.
//!
//! Must not depend on: `app`, `server` or the editor; these are pure functions of values.
//!
//! A reader takes its own version and the one before, so that the instance running when the
//! format changes can still hand over.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use oso_core::buffer::{Baseline, FileFacts, FileStamp};
use oso_core::decode::{FileInfo, LineEnding};
use oso_core::key::{KeyPress, KeySequence};
use oso_core::kill;
use oso_core::minibuffer::{HISTORY_LENGTH, MAX_ENTRY_BYTES};
use oso_core::selection::{Goal, MARK_RING_SIZE, MarkActive, MarkRing, Selection, SelectionSet};
use oso_core::window::Scroll;
use serde_json::{Map, Value, json};

use super::carry::MAX_CARRIED;
use super::{Frame, STATE_VERSION};
use crate::dev::state::goal_state;

/// The most buffers one state may open, each with a loading thread.
pub const MAX_BUFFERS: usize = 1024;

/// The most selections one state may hold. Restoring them runs on the UI thread.
pub const MAX_SELECTIONS: usize = 10_000;

/// The most pieces the kills of one state may hold, as many as one kill with every
/// selection takes.
pub const MAX_KILL_PIECES: usize = MAX_SELECTIONS;

/// The most histories one state may carry.
pub const MAX_HISTORIES: usize = 64;

/// The longest name of a history.
const MAX_HISTORY_NAME: usize = 64;

/// The most items one state may carry: each buffer's text, kill entry and history.
pub const MAX_ITEMS: usize = MAX_BUFFERS + kill::MAX_ENTRIES + MAX_HISTORIES;

/// The largest window side, in pixels: the largest texture wgpu's default
/// limits allow, since the surface follows the window.
pub const MAX_FRAME_SIDE: u32 = 8192;

/// A parsed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The export the items come from, if any.
    pub export: u64,
    /// The length of each item, in bytes.
    pub items: Vec<usize>,
    pub buffers: Vec<BufferEntry>,
    /// The position in `buffers` of the buffer the window shows.
    pub shown: usize,
    pub selections: SelectionSet,
    pub scroll: Scroll,
    pub pending_keys: Vec<KeyPress>,
    pub frame: Option<Frame>,
    /// The kill ring's entries, the newest first.
    pub kills: Vec<KillState>,
    pub histories: Vec<HistoryState>,
}

#[cfg(test)]
impl State {
    /// The path of each buffer, `None` for `*scratch*`.
    pub fn paths(&self) -> Vec<Option<PathBuf>> {
        self.buffers
            .iter()
            .map(|buffer| buffer.path.clone())
            .collect()
    }
}

/// A buffer to open again: a file's absolute path, or `None` for `*scratch*`, and the text
/// it held unsaved, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferEntry {
    pub path: Option<PathBuf>,
    pub carried: Option<Carried>,
}

/// The text a buffer held unsaved, and what it knew of its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    pub item: usize,
    pub facts: FileFacts,
}

/// A kill entry: its item, and the length of each piece in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillState {
    pub item: usize,
    pub pieces: Vec<usize>,
}

/// A history: its name, its item, and the length of each answer in it, the newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryState {
    pub name: String,
    pub item: usize,
    pub lengths: Vec<usize>,
}

/// What a carried buffer's entry says beside its item. An error if a path is not UTF-8.
pub fn facts_state(item: usize, facts: &FileFacts) -> Result<Value, String> {
    Ok(json!({
        "item": item,
        "bom": facts.info.bom,
        "line_ending": match facts.info.line_ending {
            LineEnding::Lf => "lf",
            LineEnding::Crlf => "crlf",
        },
        "binary": facts.info.binary,
        "baseline": match &facts.baseline {
            None => Value::Null,
            Some(baseline) => json!({
                "location": utf8(&baseline.location)?,
                "file": baseline.file.map(stamp_state),
            }),
        },
        "changed_on_disk": facts.changed_on_disk,
        "save_over": facts.save_over.map(stamp_state),
        "own_spelling": facts.own_spelling.as_deref().map(utf8).transpose()?,
    }))
}

fn utf8(path: &Path) -> Result<&str, String> {
    path.to_str().ok_or_else(|| {
        format!(
            "cannot hand over a path that is not UTF-8: {}",
            path.display()
        )
    })
}

fn stamp_state(stamp: FileStamp) -> Value {
    let (before_epoch, since) = match stamp.modified.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since) => (false, since),
        Err(e) => (true, e.duration()),
    };
    json!({
        "secs": since.as_secs(),
        "nanos": since.subsec_nanos(),
        "before_epoch": before_epoch,
        "len": stamp.len,
        "mode": stamp.mode,
    })
}

/// A window's selections, marks and whether the region shows, and its mark ring.
pub fn selections_state(selections: &SelectionSet) -> Value {
    json!({
        "primary": selections.primary_index(),
        "selections": selections.iter().map(|selection| json!({
            "cursor": selection.cursor,
            "mark": selection.mark,
            "goal": goal_state(selection.goal),
        })).collect::<Vec<_>>(),
        "mark_active": match selections.mark_active() {
            MarkActive::Inactive => "inactive",
            MarkActive::Active => "active",
            MarkActive::Shifted => "shifted",
        },
        "mark_ring": selections.mark_ring().iter().collect::<Vec<_>>(),
    })
}

/// Reads a state, checking everything before anything changes. There is one
/// window until splits arrive (M3), so a state with others is refused.
pub fn parse(state: &Value) -> Result<State, String> {
    let state = state.as_object().ok_or("the state is not an object")?;
    let version = match state.get("version").and_then(Value::as_u64) {
        Some(version @ (1 | STATE_VERSION)) => version,
        Some(version) => {
            return Err(format!(
                "the state has version {version}, and this build reads versions 1 and {STATE_VERSION}"
            ));
        }
        None => return Err("the state has no version".into()),
    };
    let mut items = Items::default();
    let (export, kills, histories) = if version == 1 {
        (0, Vec::new(), Vec::new())
    } else {
        items = Items::parse(state)?;
        (
            u64_of(state, "export")?,
            parse_kills(state, &mut items)?,
            parse_histories(state, &mut items)?,
        )
    };
    let entries = array(state, "buffers")?;
    if entries.len() > MAX_BUFFERS {
        return Err(format!("the state has more than {MAX_BUFFERS} buffers"));
    }
    let buffers: Vec<BufferEntry> = entries
        .iter()
        .map(|entry| parse_buffer(entry, &mut items))
        .collect::<Result<_, _>>()?;
    let files: HashSet<_> = buffers.iter().filter_map(|b| b.path.as_ref()).collect();
    if files.len() != buffers.iter().filter(|b| b.path.is_some()).count() {
        return Err("the state opens one file twice".into());
    }
    if buffers
        .iter()
        .filter(|b| b.path.is_none() && b.carried.is_some())
        .count()
        > 1
    {
        return Err("the state carries the text of two buffers with no file".into());
    }
    let windows = array(state, "windows")?;
    let ([window], Some(0)) = (&windows[..], state.get("selected").and_then(Value::as_u64)) else {
        return Err("the state must have one window, selected".into());
    };
    let window = window.as_object().ok_or("a window is not an object")?;
    let shown = entries
        .iter()
        .position(|entry| entry.get("id") == window.get("buffer"))
        .ok_or("the window shows a buffer the state does not have")?;
    let scroll = window
        .get("scroll")
        .and_then(Value::as_object)
        .ok_or("a window has no scroll position")?;
    Ok(State {
        export,
        items: items.lengths,
        buffers,
        shown,
        selections: parse_selections(window)?,
        scroll: Scroll {
            top: size(scroll, "top")?,
            left: size(scroll, "left")?,
        },
        pending_keys: parse_keys(state)?,
        frame: parse_frame(state)?,
        kills,
        histories,
    })
}

/// The lengths of the items, and which have been claimed, each by one thing only.
#[derive(Default)]
struct Items {
    lengths: Vec<usize>,
    claimed: HashSet<usize>,
}

impl Items {
    fn parse(state: &Map<String, Value>) -> Result<Self, String> {
        let items = array(state, "items")?;
        if items.len() > MAX_ITEMS {
            return Err(format!("the state has more than {MAX_ITEMS} items"));
        }
        let lengths: Vec<usize> = items
            .iter()
            .map(|len| {
                len.as_u64()
                    .and_then(|len| usize::try_from(len).ok())
                    .ok_or_else(|| "an item's length is not a size".to_owned())
            })
            .collect::<Result<_, _>>()?;
        let total = lengths
            .iter()
            .try_fold(0usize, |total, &len| total.checked_add(len))
            .filter(|&total| total <= MAX_CARRIED);
        if total.is_none() {
            return Err(format!("the state carries more than {MAX_CARRIED} bytes"));
        }
        Ok(Self {
            lengths,
            claimed: HashSet::new(),
        })
    }

    /// Claims the item named in `object`, whose length must be the sum of `parts`, if given.
    fn claim(
        &mut self,
        object: &Map<String, Value>,
        parts: Option<&[usize]>,
    ) -> Result<usize, String> {
        let item = size(object, "item")?;
        let Some(&len) = self.lengths.get(item) else {
            return Err(format!("the state has no item {item}"));
        };
        if !self.claimed.insert(item) {
            return Err(format!("item {item} is claimed twice"));
        }
        let sum = |parts: &[usize]| {
            parts
                .iter()
                .try_fold(0usize, |sum, &part| sum.checked_add(part))
        };
        if parts.is_some_and(|parts| sum(parts) != Some(len)) {
            return Err(format!("item {item}'s parts do not add up to its length"));
        }
        Ok(item)
    }
}

fn parse_kills(state: &Map<String, Value>, items: &mut Items) -> Result<Vec<KillState>, String> {
    let entries = array(state, "kills")?;
    if entries.len() > kill::MAX_ENTRIES {
        return Err(format!(
            "the state has more than {} kills",
            kill::MAX_ENTRIES
        ));
    }
    let mut pieces_in_all = 0;
    entries
        .iter()
        .map(|entry| {
            let entry = entry.as_object().ok_or("a kill is not an object")?;
            let pieces = sizes(entry, "pieces")?;
            pieces_in_all += pieces.len();
            if pieces.is_empty() || pieces_in_all > MAX_KILL_PIECES {
                return Err(format!(
                    "a kill has no pieces, or the kills more than {MAX_KILL_PIECES}"
                ));
            }
            let item = items.claim(entry, Some(&pieces))?;
            Ok(KillState { item, pieces })
        })
        .collect()
}

fn parse_histories(
    state: &Map<String, Value>,
    items: &mut Items,
) -> Result<Vec<HistoryState>, String> {
    let entries = array(state, "histories")?;
    if entries.len() > MAX_HISTORIES {
        return Err(format!("the state has more than {MAX_HISTORIES} histories"));
    }
    entries
        .iter()
        .map(|entry| {
            let entry = entry.as_object().ok_or("a history is not an object")?;
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty() && name.len() <= MAX_HISTORY_NAME)
                .ok_or("a history has no name")?;
            let lengths = sizes(entry, "lengths")?;
            if lengths.len() > HISTORY_LENGTH || lengths.iter().any(|&len| len > MAX_ENTRY_BYTES) {
                return Err(format!("history {name:?} holds more than a history keeps"));
            }
            let item = items.claim(entry, Some(&lengths))?;
            Ok(HistoryState {
                name: name.to_owned(),
                item,
                lengths,
            })
        })
        .collect()
}

fn parse_buffer(entry: &Value, items: &mut Items) -> Result<BufferEntry, String> {
    let entry = entry.as_object().ok_or("a buffer is not an object")?;
    if entry.get("id").and_then(Value::as_u64).is_none() {
        return Err("a buffer has no id".into());
    }
    let path = optional_path(entry, "path")?;
    let carried = match entry.get("text") {
        None | Some(Value::Null) => None,
        Some(Value::Object(text)) => Some(Carried {
            item: items.claim(text, None)?,
            facts: parse_facts(text)?,
        }),
        Some(_) => return Err("a buffer's text is not an object".into()),
    };
    Ok(BufferEntry { path, carried })
}

fn parse_facts(text: &Map<String, Value>) -> Result<FileFacts, String> {
    let line_ending = match text.get("line_ending").and_then(Value::as_str) {
        Some("lf") => LineEnding::Lf,
        Some("crlf") => LineEnding::Crlf,
        _ => return Err("a buffer's line ending is not understood".into()),
    };
    let baseline = match text.get("baseline") {
        None | Some(Value::Null) => None,
        Some(Value::Object(baseline)) => Some(Baseline {
            location: optional_path(baseline, "location")?.ok_or("a baseline has no location")?,
            file: optional_stamp(baseline, "file")?,
        }),
        Some(_) => return Err("a buffer's baseline is not an object".into()),
    };
    Ok(FileFacts {
        info: FileInfo {
            bom: flag(text, "bom")?,
            line_ending,
            binary: flag(text, "binary")?,
        },
        baseline,
        changed_on_disk: flag(text, "changed_on_disk")?,
        save_over: optional_stamp(text, "save_over")?,
        own_spelling: optional_path(text, "own_spelling")?,
    })
}

fn optional_stamp(object: &Map<String, Value>, name: &str) -> Result<Option<FileStamp>, String> {
    let stamp = match object.get(name) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(stamp)) => stamp,
        Some(_) => return Err(format!("{name} is not a file stamp")),
    };
    let since = Duration::new(
        u64_of(stamp, "secs")?,
        u32::try_from(u64_of(stamp, "nanos")?)
            .ok()
            .filter(|&nanos| nanos < 1_000_000_000)
            .ok_or("a stamp's nanos are not a fraction")?,
    );
    let modified = if flag(stamp, "before_epoch")? {
        SystemTime::UNIX_EPOCH.checked_sub(since)
    } else {
        SystemTime::UNIX_EPOCH.checked_add(since)
    }
    .ok_or("a stamp's time is out of range")?;
    Ok(Some(FileStamp {
        modified,
        len: u64_of(stamp, "len")?,
        mode: u32::try_from(u64_of(stamp, "mode")?).map_err(|_| "a stamp's mode is too large")?,
    }))
}

/// An absolute path, or `None` if it is missing or null.
fn optional_path(object: &Map<String, Value>, name: &str) -> Result<Option<PathBuf>, String> {
    let path = match object.get(name) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(path)) => PathBuf::from(path),
        Some(_) => return Err(format!("a buffer's {name} is not a string")),
    };
    if !path.is_absolute() {
        return Err(format!("a buffer's {name} is not absolute"));
    }
    Ok(Some(path))
}

fn parse_selections(window: &Map<String, Value>) -> Result<SelectionSet, String> {
    let selections = array(window, "selections")?;
    if selections.len() > MAX_SELECTIONS {
        return Err(format!(
            "a window has more than {MAX_SELECTIONS} selections"
        ));
    }
    let selections = selections
        .iter()
        .map(|selection| {
            let selection = selection
                .as_object()
                .ok_or("a selection is not an object")?;
            let goal = match selection.get("goal") {
                None | Some(Value::Null) => None,
                Some(Value::String(goal)) if goal == "line-end" => Some(Goal::LineEnd),
                Some(Value::Object(goal)) => Some(Goal::Column(size(goal, "column")?)),
                Some(_) => return Err("a selection's goal is not understood".to_owned()),
            };
            let mark = match selection.get("mark") {
                None | Some(Value::Null) => None,
                Some(_) => Some(size(selection, "mark")?),
            };
            Ok(Selection {
                cursor: size(selection, "cursor")?,
                mark,
                goal,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut set = SelectionSet::from_parts(selections, size(window, "primary")?)
        .ok_or_else(|| "a window's primary selection does not exist".to_owned())?;
    let active = match window.get("mark_active").and_then(Value::as_str) {
        None | Some("inactive") => MarkActive::Inactive,
        Some("active") => MarkActive::Active,
        Some("shifted") => MarkActive::Shifted,
        Some(_) => return Err("a window's mark_active is not understood".into()),
    };
    set.set_mark_active(active);
    let ring = match window.get("mark_ring") {
        None | Some(Value::Null) => Vec::new(),
        Some(_) => sizes(window, "mark_ring")?,
    };
    if ring.len() > MARK_RING_SIZE {
        return Err(format!("a mark ring holds at most {MARK_RING_SIZE} marks"));
    }
    Ok(set.with_ring(MarkRing::from_newest(ring)))
}

fn parse_keys(state: &Map<String, Value>) -> Result<Vec<KeyPress>, String> {
    let keys = state
        .get("pending_keys")
        .and_then(Value::as_str)
        .ok_or("the state has no pending keys")?;
    let KeySequence(keys) = keys.parse().map_err(|e| format!("{e}"))?;
    Ok(keys)
}

fn parse_frame(state: &Map<String, Value>) -> Result<Option<Frame>, String> {
    let frame = match state.get("frame") {
        None | Some(Value::Null) => return Ok(None),
        Some(frame) => frame.as_object().ok_or("the frame is not an object")?,
    };
    let coordinate = |name: &str| match frame.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| format!("the frame's {name} is not a position")),
    };
    let side = |name: &str| {
        frame
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| (1..=MAX_FRAME_SIDE).contains(value))
            .ok_or_else(|| format!("the frame's {name} is not a size"))
    };
    Ok(Some(Frame {
        position: coordinate("x")?.zip(coordinate("y")?),
        width: side("width")?,
        height: side("height")?,
    }))
}

fn array<'a>(object: &'a Map<String, Value>, name: &str) -> Result<&'a Vec<Value>, String> {
    object
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("the state has no {name}"))
}

fn sizes(object: &Map<String, Value>, name: &str) -> Result<Vec<usize>, String> {
    array(object, name)?
        .iter()
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| format!("{name} holds something that is not a size"))
        })
        .collect()
}

fn size(object: &Map<String, Value>, name: &str) -> Result<usize, String> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("{name} is not a size"))
}

fn u64_of(object: &Map<String, Value>, name: &str) -> Result<u64, String> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{name} is not a number"))
}

fn flag(object: &Map<String, Value>, name: &str) -> Result<bool, String> {
    object
        .get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{name} is not true or false"))
}
