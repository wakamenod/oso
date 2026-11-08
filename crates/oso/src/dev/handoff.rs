//! The state one instance hands to the next on a restart, and whether it may
//! hand over and go: `export-state`, `import-state`, `check-handover` and the
//! guard on `exit` (ADR 0023, ADR 0047, ADR 0055, ADR 0059).
//!
//! Must not depend on: `app`, `server`, winit or sockets; these are plain
//! functions of the editor state, and the caller opens the files.
//!
//! The state holds paths and positions and never any text, so a secret file's
//! content cannot leave through it (ADR 0044), and a response line is enough
//! to carry it (ADR 0032). It holds no position in such a file either, so a window
//! showing one starts over from the top (ADR 0051, ADR 0058). The new instance reads
//! each file from disk, so a
//! buffer with text no file holds cannot be handed over. Exporting one is an
//! error that names it and quotes nothing, and the old instance stays
//! (ADR 0051, ADR 0055). Carrying the text is M2-15.
//!
//! A generation names the text an instance holds, so that it can refuse to exit
//! if that has changed since a caller last looked. See [`generation`].
//!
//! While a buffer that may hold secrets is shown, an instance does not hand over, for one
//! reason whatever its buffers are, so that the answer says nothing of that buffer. Keys in
//! progress typed around one are not handed over.
//!
//! Parsing changes nothing, so a state with another version or a bad field
//! leaves the new instance clean, and the old one is kept.

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId, Verdict};
use oso_core::display_map::DisplayPoint;
use oso_core::editor::{Editor, SCRATCH};
use oso_core::key::{KeyPress, KeySequence};
use oso_core::keymap::display_keys;
use oso_core::selection::{Goal, Selection, SelectionSet};
use oso_core::sum_tree::Bias;
use oso_core::window::{Scroll, Window};
use serde_json::{Map, Value, json};

use super::protocol::{EXIT_REFUSED, MAX_LINE, Reply};
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
    if shows_judged_secret(editor) {
        return Err(SECRET_SHOWN.into());
    }
    if editor.prompt().is_some() {
        return Err(PROMPTING.into());
    }
    if editor.isearch().is_some() {
        return Err(SEARCHING.into());
    }
    let mut buffers = Vec::new();
    for (id, buffer) in editor.buffers() {
        buffers.push(buffer_state(id, buffer)?);
    }
    // Internal buffers are not handed over (ADR 0051). A window on one with nothing to go
    // back to comes back on `*scratch*`, which this entry stands for.
    for window in editor.windows() {
        if editor.shown(window).is_internal() && behind_internal(editor, window).is_none() {
            buffers.push(json!({ "id": window.buffer().get(), "path": null }));
        }
    }
    let at_the_top = SelectionSet::default();
    let windows: Vec<Value> = editor
        .windows()
        .map(|window| {
            let (buffer, selections, scroll) = handed_over(editor, window);
            // Positions still waiting came in with the import, and so say nothing
            // of the text, even while a file counts as secret until it is judged.
            let waiting = restore.filter(|r| r.buffer == buffer);
            let withheld = editor
                .buffer(buffer)
                .is_none_or(|shown| shown.is_secret() || shown.is_internal());
            let (selections, scroll) = match waiting {
                Some(restore) => (&restore.selections, restore.scroll),
                None if withheld => (&at_the_top, Scroll::default()),
                None => (selections, scroll),
            };
            json!({
                "buffer": buffer.get(),
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
        "generation": generation(editor),
        "buffers": buffers,
        "windows": windows,
        "selected": editor.selected_window(),
        "pending_keys": if editor.keys_withheld() {
            String::new()
        } else {
            display_keys(pending).to_string()
        },
        // The theme drawn in, so a restart right after a switch does not wait for the
        // remembered one to be written (ADR 0089).
        "theme": editor.theme_name(),
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

/// Where `window` goes back to, if it is on an internal buffer and has somewhere to go that
/// cannot hold secrets.
fn behind_internal<'a>(
    editor: &'a Editor,
    window: &Window,
) -> Option<(BufferId, &'a SelectionSet, Scroll)> {
    // One that may hold secrets is not shown again by a restart: the window comes back on
    // `*scratch*`, and the buffer is handed over unshown.
    editor
        .shown(window)
        .is_internal()
        .then(|| editor.place_behind_internal())
        .flatten()
        .filter(|(buffer, ..)| {
            editor
                .buffer(*buffer)
                .is_some_and(|buffer| !buffer.is_secret())
        })
}

/// What `window` hands over: what it shows, or from an internal buffer, where it goes back
/// to, since no key could reach that again otherwise.
fn handed_over<'a>(editor: &'a Editor, window: &'a Window) -> (BufferId, &'a SelectionSet, Scroll) {
    behind_internal(editor, window).unwrap_or((
        window.buffer(),
        &window.selections,
        window.scroll(),
    ))
}

fn buffer_state(id: BufferId, buffer: &Buffer) -> Result<Value, String> {
    Ok(json!({ "id": id.get(), "path": file_to_read_again(buffer)? }))
}

/// The path a new instance reads `buffer` from, or `None` for an empty
/// `*scratch*`, which needs nothing. An error, naming the buffer and quoting
/// none of its text, if the new instance would not end up with the same text.
fn file_to_read_again(buffer: &Buffer) -> Result<Option<&str>, String> {
    match buffer.path() {
        // The disk may end up holding what this buffer no longer does.
        Some(_) if buffer.is_saving() => Err(format!(
            "cannot hand over {}: it is being saved",
            buffer.name()
        )),
        Some(_) if buffer.is_modified() => Err(format!(
            "cannot hand over {}: it has unsaved edits",
            buffer.name()
        )),
        // Read again by its path, it would be judged by its name alone (ADR 0074).
        Some(_) if buffer.verdict() == Verdict::Yanked => Err(format!(
            "cannot hand over {}: it holds a secret kill yanked into it",
            buffer.name()
        )),
        Some(path) => path.to_str().map(Some).ok_or_else(|| {
            format!(
                "cannot hand over a path that is not UTF-8: {}",
                path.display()
            )
        }),
        None if buffer.text().is_empty() && buffer.name() == SCRATCH => Ok(None),
        None => Err(format!(
            "cannot hand over {}: it has no file to read again",
            buffer.name()
        )),
    }
}

/// The result of `check-handover`: the instance's generation, or why its text
/// would not survive a restart. These are the buffers `export` would refuse,
/// without building a state, so that the supervisor can ask before it opens a new
/// window.
pub fn check(editor: &Editor) -> Reply {
    if shows_judged_secret(editor) {
        return Err(SECRET_SHOWN.into());
    }
    for (_, buffer) in editor.buffers() {
        file_to_read_again(buffer)?;
    }
    if editor.preedit().is_some() {
        return Err(COMPOSING.into());
    }
    if editor.prompt().is_some() {
        return Err(PROMPTING.into());
    }
    if editor.isearch().is_some() {
        return Err(SEARCHING.into());
    }
    Ok(json!({ "generation": generation(editor) }))
}

/// Why a restart waits for a composition to end: its text is in no buffer, so a new
/// instance would lose it (ADR 0069).
const COMPOSING: &str = "cannot hand over: a composition is open";

/// Why a restart waits for every prompt to close: it is no unsaved work, but a new
/// instance would take it away from the person mid-answer (ADR 0072).
const PROMPTING: &str = "cannot hand over: a prompt is open";

/// Why a restart waits for isearch to end: a new instance would take the search away
/// mid-typing (ADR 0082).
const SEARCHING: &str = "cannot hand over: a search is under way";

/// Why a restart waits while a buffer that may hold secrets is shown: the reason is the same
/// whatever that buffer holds.
const SECRET_SHOWN: &str = "cannot hand over while a buffer that may hold secrets is shown";

/// Whether a window shows a buffer judged to may hold secrets. A file not judged yet does not
/// count: it is loading, and its positions are the ones it came in with.
fn shows_judged_secret(editor: &Editor) -> bool {
    editor.windows().any(|window| {
        let verdict = editor.shown(window).verdict();
        verdict.is_withheld() && verdict != Verdict::Unjudged
    })
}

/// Names what an instance holds that a file does not: which buffers exist, and
/// the content of each modified one. It is not a counter, as undoing an edit
/// brings the old value back, and only the instance that made it compares it.
///
/// A buffer that is not modified counts only by its id, since each piece of a
/// load gets a new content id and a large file may still be arriving. So a save
/// does not change the generation either: that text is on the disk.
pub fn generation(editor: &Editor) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (id, buffer) in editor.buffers() {
        id.hash(&mut hasher);
        buffer
            .is_modified()
            .then(|| buffer.content_id())
            .hash(&mut hasher);
    }
    hasher.finish()
}

/// Whether `exit` may go ahead. Without a generation it always may: that is a
/// person or an agent saying so. With one, only if nothing has changed since and
/// no save is running, as a new instance may already have read the old file, and not
/// while a buffer that may hold secrets is shown.
pub fn exit_allowed(editor: &Editor, expected: Option<u64>) -> Result<(), String> {
    match expected {
        Some(_) if shows_judged_secret(editor) => Err(format!("{EXIT_REFUSED}: {SECRET_SHOWN}")),
        Some(_) if editor.buffers().any(|(_, buffer)| buffer.is_saving()) => {
            Err(format!("{EXIT_REFUSED}: a save is running"))
        }
        Some(_) if editor.preedit().is_some() => {
            Err(format!("{EXIT_REFUSED}: a composition is open"))
        }
        Some(_) if editor.prompt().is_some() => Err(format!("{EXIT_REFUSED}: a prompt is open")),
        Some(_) if editor.isearch().is_some() => {
            Err(format!("{EXIT_REFUSED}: a search is under way"))
        }
        Some(expected) if expected != generation(editor) => Err(format!(
            "{EXIT_REFUSED}: the buffers have changed since generation {expected}"
        )),
        _ => Ok(()),
    }
}

/// Whether this instance may take over from another. It may only if it has
/// opened nothing and its `*scratch*` is empty, as the imported buffers would hide
/// any text there. Internal buffers do not count, but one in view does, since the
/// window then has no `*scratch*` to start from.
pub fn import_allowed(editor: &Editor) -> Result<(), String> {
    let (_, scratch) = editor.current();
    if scratch.is_internal() {
        return Err(format!("this instance's window has no {SCRATCH} in view"));
    }
    if editor.buffers().count() > 1 || scratch.name() != SCRATCH {
        return Err("this instance has already opened something".into());
    }
    if !scratch.text().is_empty() {
        return Err(format!(
            "this instance has text in {SCRATCH}, which the import would hide"
        ));
    }
    Ok(())
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
    /// The theme the previous instance drew in, if it said.
    pub theme: Option<String>,
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
        theme: parse_theme(state)?,
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
            // Marks are not handed over until M2-15 (ADR 0051, ADR 0071).
            Ok(Selection {
                cursor: size(selection, "cursor")?,
                mark: None,
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

/// The theme's name, if the state has one: a short name of lower-case letters, digits and
/// `-`, as the built-in themes are named. A state from before themes has none.
fn parse_theme(state: &Map<String, Value>) -> Result<Option<String>, String> {
    match state.get("theme") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name))
            if (1..=32).contains(&name.len())
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') =>
        {
            Ok(Some(name.clone()))
        }
        Some(_) => Err("the theme is not a theme's name".into()),
    }
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
                    mark: None,
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
mod internal_tests;
#[cfg(test)]
mod tests;
