//! The state one instance hands to the next on a restart: `export-state` and
//! `import-state` (ADR 0023, ADR 0047).
//!
//! Must not depend on: `app`, `server`, winit or sockets; these are plain
//! functions of the editor state, and the caller opens the files.
//!
//! The state holds paths and positions and never any text, so a secret file's
//! content cannot leave through it (ADR 0044), and a response line is enough
//! to carry it (ADR 0032). The new instance reads each file from disk. Nothing
//! unsaved exists until M2, when text may join the format for buffers that are
//! not secret. A path-less buffer with text cannot occur before then either,
//! so exporting one is an error rather than a silent loss.
//!
//! Parsing changes nothing, so a state with another version or a bad field
//! leaves the new instance clean, and the old one is kept.

use std::collections::HashSet;
use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::display_map::DisplayPoint;
use oso_core::editor::{Editor, SCRATCH};
use oso_core::key::{KeyPress, KeySequence};
use oso_core::keymap::display_keys;
use oso_core::selection::{Goal, Selection, SelectionSet};
use oso_core::sum_tree::Bias;
use oso_core::window::Scroll;
use serde_json::{Map, Value, json};

use super::protocol::{MAX_LINE, Reply};
use super::state::goal_state;

/// The version of the state format. Bump it on any change that the previous
/// reader would get wrong.
pub const STATE_VERSION: u64 = 1;

/// The longest state, leaving room for the request line that carries it.
const MAX_STATE: usize = MAX_LINE - 256;

/// The most buffers one state may open, each with a loading thread.
const MAX_BUFFERS: usize = 1024;

/// The most selections one state may hold. Restoring them runs on the UI thread.
const MAX_SELECTIONS: usize = 10_000;

/// The largest window side, in pixels: the largest texture wgpu's default
/// limits allow, since the surface follows the window.
const MAX_FRAME_SIDE: u32 = 8192;

/// The OS window's inner size and, where the window system tells, its place on
/// the screen, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub position: Option<(i32, i32)>,
    pub width: u32,
    pub height: u32,
}

/// The result of `export-state`. `restore` is what the window has yet to take
/// from the previous instance: if its file is still loading, those positions
/// are the real ones.
pub fn export(
    editor: &Editor,
    pending: &[KeyPress],
    frame: Option<Frame>,
    restore: Option<&Restore>,
) -> Reply {
    let mut buffers = Vec::new();
    for (id, buffer) in editor.buffers() {
        buffers.push(buffer_state(id, buffer)?);
    }
    let windows: Vec<Value> = editor
        .windows()
        .map(|window| {
            let (selections, scroll) = match restore.filter(|r| r.buffer == window.buffer()) {
                Some(restore) => (&restore.selections, restore.scroll),
                None => (&window.selections, window.scroll()),
            };
            json!({
                "buffer": window.buffer().get(),
                "primary": selections.primary_index(),
                "selections": selections.iter().map(|selection| json!({
                    "cursor": selection.cursor,
                    "goal": goal_state(selection.goal),
                })).collect::<Vec<_>>(),
                "scroll": { "top": scroll.top, "left": scroll.left },
            })
        })
        .collect();
    let state = json!({
        "version": STATE_VERSION,
        "buffers": buffers,
        "windows": windows,
        "selected": editor.selected_window(),
        "pending_keys": display_keys(pending).to_string(),
        "frame": frame.map(|frame| json!({
            "x": frame.position.map(|position| position.0),
            "y": frame.position.map(|position| position.1),
            "width": frame.width,
            "height": frame.height,
        })),
    });
    let size = state.to_string().len();
    if size > MAX_STATE {
        return Err(format!(
            "the state takes {size} bytes, more than the {MAX_STATE} it may"
        ));
    }
    Ok(state)
}

fn buffer_state(id: BufferId, buffer: &Buffer) -> Result<Value, String> {
    let path = match buffer.path() {
        Some(path) => Some(path.to_str().ok_or_else(|| {
            format!(
                "cannot hand over a path that is not UTF-8: {}",
                path.display()
            )
        })?),
        None if buffer.text().is_empty() && buffer.name() == SCRATCH => None,
        None => {
            return Err(format!(
                "cannot hand over {}: it has no file to read again",
                buffer.name()
            ));
        }
    };
    Ok(json!({ "id": id.get(), "path": path }))
}

/// A parsed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The buffers to open again: a file's absolute path, or `None` for
    /// `*scratch*`.
    pub buffers: Vec<Option<PathBuf>>,
    /// The position in `buffers` of the buffer the window shows.
    pub shown: usize,
    pub selections: SelectionSet,
    pub scroll: Scroll,
    pub pending_keys: Vec<KeyPress>,
    pub frame: Option<Frame>,
}

/// Reads a state, checking everything before anything changes. There is one
/// window until splits arrive (M3), so a state with others is refused.
pub fn parse(state: &Value) -> Result<State, String> {
    let state = state.as_object().ok_or("the state is not an object")?;
    match state.get("version").and_then(Value::as_u64) {
        Some(STATE_VERSION) => {}
        Some(version) => {
            return Err(format!(
                "the state has version {version}, and this build reads version {STATE_VERSION}"
            ));
        }
        None => return Err("the state has no version".into()),
    }
    let entries = array(state, "buffers")?;
    if entries.len() > MAX_BUFFERS {
        return Err(format!("the state has more than {MAX_BUFFERS} buffers"));
    }
    let buffers: Vec<Option<PathBuf>> =
        entries.iter().map(parse_buffer).collect::<Result<_, _>>()?;
    let files: HashSet<_> = buffers.iter().flatten().collect();
    if files.len() != buffers.iter().flatten().count() {
        return Err("the state opens one file twice".into());
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
        buffers,
        shown,
        selections: parse_selections(window)?,
        scroll: Scroll {
            top: size(scroll, "top")?,
            left: size(scroll, "left")?,
        },
        pending_keys: parse_keys(state)?,
        frame: parse_frame(state)?,
    })
}

fn parse_buffer(entry: &Value) -> Result<Option<PathBuf>, String> {
    let entry = entry.as_object().ok_or("a buffer is not an object")?;
    if entry.get("id").and_then(Value::as_u64).is_none() {
        return Err("a buffer has no id".into());
    }
    let path = match entry.get("path") {
        None | Some(Value::Null) => None,
        Some(Value::String(path)) => Some(PathBuf::from(path)),
        Some(_) => return Err("a buffer's path is not a string".into()),
    };
    if path.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err("a buffer's path is not absolute".into());
    }
    Ok(path)
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
            Ok(Selection {
                cursor: size(selection, "cursor")?,
                goal,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    SelectionSet::from_parts(selections, size(window, "primary")?)
        .ok_or_else(|| "a window's primary selection does not exist".to_owned())
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

fn size(object: &Map<String, Value>, name: &str) -> Result<usize, String> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("{name} is not a size"))
}

/// Positions waiting for their file to load, since a cursor cannot sit in text
/// that has not arrived. The file may also have changed since the export, so
/// they are moved to the nearest place that exists.
#[derive(Debug)]
pub struct Restore {
    pub buffer: BufferId,
    pub selections: SelectionSet,
    pub scroll: Scroll,
}

impl Restore {
    /// Moves the window to the positions if the buffer holds them by now.
    /// Returns whether this is over: restored, or the window has moved on.
    pub fn try_apply(&self, editor: &mut Editor) -> bool {
        let (window, buffer) = editor.current_mut();
        if window.buffer() != self.buffer {
            return true;
        }
        let needed = self.selections.iter().map(|s| s.cursor).max().unwrap_or(0);
        if buffer.loading().is_some() && buffer.text().len() < needed {
            return false;
        }
        let display = buffer.display();
        let selections = self
            .selections
            .iter()
            .map(|selection| {
                let point: DisplayPoint = display.offset_to_display_point(selection.cursor);
                Selection {
                    cursor: display.display_point_to_offset(point, Bias::Left),
                    goal: selection.goal,
                }
            })
            .collect();
        let selections = SelectionSet::from_parts(selections, self.selections.primary_index())
            .unwrap_or_else(|| window.selections.clone());
        editor.restore_positions(selections, self.scroll);
        true
    }
}

#[cfg(test)]
mod tests;
