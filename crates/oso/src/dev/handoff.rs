//! The state one instance hands to the next on a restart, and whether it may
//! hand over and go: `export-state`, `import-state`, `check-handover` and the
//! guard on `exit` (ADR 0023, ADR 0047, ADR 0055, ADR 0059, ADR 0090).
//!
//! Must not depend on: `app`, `server`, winit or sockets; these are plain
//! functions of the editor state, and the caller opens the files. Only [`carry`]
//! starts threads, to copy and build the texts off the UI thread.
//!
//! A buffer without edits is handed over by its path, and the new instance reads
//! it from disk and judges it like any file it opens. The text of a modified
//! buffer, the kill ring and the histories are carried as items, which the state
//! names by length only: `export-text` and `import-text` move them in pieces
//! ([`carry`]), so the state still fits in a line (ADR 0032). The text of a buffer
//! that may hold secrets is never carried (ADR 0044), and such a buffer with
//! edits holds the restart, named and quoting nothing (ADR 0051, ADR 0055). Nor
//! is any position in such a file, so a window showing one starts over from the
//! top (ADR 0058).
//!
//! A generation names what an instance holds that a file does not, so that it can
//! refuse to exit if that has changed since a caller last looked. See [`generation`].
//!
//! While a buffer that may hold secrets is shown, an instance does not hand over, for one
//! reason whatever its buffers are, so that the answer says nothing of that buffer. Keys in
//! progress typed around one are not handed over.
//!
//! Parsing changes nothing, so a state with another version or a bad field
//! leaves the new instance clean, and the old one is kept.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use oso_core::buffer::{Buffer, BufferId, Verdict};
use oso_core::display_map::{DisplayPoint, DisplaySnapshot};
use oso_core::editor::{Editor, SCRATCH};
use oso_core::key::KeyPress;
use oso_core::keymap::display_keys;
use oso_core::kill::KillEntry;
use oso_core::rope::Rope;
use oso_core::selection::{MarkRing, Selection, SelectionSet};
use oso_core::sum_tree::Bias;
use oso_core::window::{Scroll, Window};
use serde_json::{Map, Value, json};

use super::protocol::{EXIT_REFUSED, MAX_LINE, Reply};

pub mod carry;
mod format;

use carry::{Exported, Item, MAX_CARRIED};

#[cfg(test)]
use format::{MAX_BUFFERS, MAX_FRAME_SIDE, MAX_SELECTIONS};
pub use format::{MAX_ITEMS, State, parse};
use format::{MAX_KILL_PIECES, facts_state, selections_state};

/// The version of the state format. Bump it on any change that the previous
/// reader would get wrong, and keep reading the one before.
pub const STATE_VERSION: u64 = 3;

/// The longest state, leaving room for the request line that carries it.
const MAX_STATE: usize = MAX_LINE - 256;

/// The OS window's inner size and, where the window system tells, its place on
/// the screen, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub position: Option<(i32, i32)>,
    pub width: u32,
    pub height: u32,
}

/// The result of `export-state`, and the snapshot `export-text` reads its items from.
/// `restore` is what the window has yet to take from the previous instance: if its file
/// is still loading, those positions are the real ones. `number` names this export.
pub fn export(
    editor: &Editor,
    pending: &[KeyPress],
    frame: Option<Frame>,
    restore: Option<&Restore>,
    number: u64,
) -> Result<(Value, Exported), String> {
    if shows_judged_secret(editor) {
        return Err(SECRET_SHOWN.into());
    }
    if editor.prompt().is_some() {
        return Err(PROMPTING.into());
    }
    if editor.isearch().is_some() {
        return Err(SEARCHING.into());
    }
    carried_bytes(editor)?;
    let mut items = Vec::new();
    let mut buffers = Vec::new();
    // A file the agent channel is still opening goes with its open: the next instance would
    // hold it as an ordinary buffer, unchecked (ADR 0094).
    for (id, buffer) in editor
        .buffers()
        .filter(|(_, buffer)| !buffer.agent_pending())
    {
        buffers.push(buffer_state(id, buffer, &mut items)?);
    }
    // Internal buffers are not handed over (ADR 0051). A window on one with nothing to go
    // back to comes back on `*scratch*`, which this entry stands for.
    for window in editor.windows() {
        if editor.shown(window).is_internal() && behind_internal(editor, window).is_none() {
            buffers.push(json!({ "id": window.buffer().get(), "path": null }));
        }
    }
    let histories = histories_state(editor, &mut items);
    // The buffers' texts and the histories come first; kills fill what is left.
    let room = MAX_CARRIED.saturating_sub(items.iter().map(Item::len).sum());
    let kills = kills_state(editor, room, &mut items);
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
            let mut state = selections_state(selections);
            state["buffer"] = json!(buffer.get());
            state["scroll"] = json!({ "start": scroll.start, "left": scroll.left });
            state
        })
        .collect();
    let state = json!({
        "version": STATE_VERSION,
        "generation": generation(editor),
        "export": number,
        "items": items.iter().map(Item::len).collect::<Vec<_>>(),
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
        "kills": kills,
        "histories": histories,
    });
    let size = state.to_string().len();
    if size > MAX_STATE {
        return Err(format!(
            "the state takes {size} bytes, more than the {MAX_STATE} it may"
        ));
    }
    Ok((state, Exported::new(number, items)))
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

fn buffer_state(id: BufferId, buffer: &Buffer, items: &mut Vec<Item>) -> Result<Value, String> {
    let (path, text) = match handed(buffer)? {
        Handed::Nothing => (None, Value::Null),
        Handed::Path(path) => (Some(path), Value::Null),
        Handed::Carried(path) => {
            items.push(Item(vec![buffer.text().clone()]));
            (path, facts_state(items.len() - 1, &buffer.file_facts())?)
        }
    };
    let mut entry = json!({ "id": id.get(), "path": path, "text": text });
    // Said only when true, so that a state from before it reads the same (ADR 0085).
    if buffer.opened_by_agent() {
        entry["opened_by_agent"] = Value::Bool(true);
    }
    // Also only when true (ADR 0100).
    if buffer.wraps() {
        entry["wraps"] = Value::Bool(true);
    }
    Ok(entry)
}

/// How a buffer is handed over.
enum Handed<'a> {
    /// An empty `*scratch*`, which needs nothing.
    Nothing,
    /// By the path a new instance reads it from.
    Path(&'a str),
    /// With its text, and the path it visits, if any.
    Carried(Option<&'a str>),
}

/// How `buffer` is handed over, or an error naming it and quoting none of its text if the
/// new instance would not end up with the same text.
fn handed(buffer: &Buffer) -> Result<Handed<'_>, String> {
    let path = buffer.path().map(|path| {
        path.to_str().ok_or_else(|| {
            format!(
                "cannot hand over a path that is not UTF-8: {}",
                path.display()
            )
        })
    });
    let scratch = path.is_none() && buffer.name() == SCRATCH;
    // An emptied `*scratch*` holds nothing to carry, though it counts as modified.
    let carried = if scratch {
        !buffer.text().is_empty()
    } else {
        buffer.is_modified()
    };
    match path {
        // The disk may end up holding what this buffer no longer does.
        Some(_) if buffer.is_saving() => Err(format!(
            "cannot hand over {}: it is being saved",
            buffer.name()
        )),
        // A file not judged yet counts too, until it is.
        _ if carried && buffer.is_secret() => Err(format!(
            "cannot hand over {}: it has unsaved edits and may hold secrets",
            buffer.name()
        )),
        _ if carried && buffer.recovery_stands() => Err(format!(
            "cannot hand over {}: it holds an emergency copy not saved yet",
            buffer.name()
        )),
        Some(path) if carried => Ok(Handed::Carried(Some(path?))),
        None if carried && scratch => Ok(Handed::Carried(None)),
        // Read again by its path, it would be judged by its name alone (ADR 0074).
        Some(_) if buffer.verdict() == Verdict::Yanked => Err(format!(
            "cannot hand over {}: it holds a secret kill yanked into it",
            buffer.name()
        )),
        Some(path) => Ok(Handed::Path(path?)),
        None if scratch => Ok(Handed::Nothing),
        None => Err(format!(
            "cannot hand over {}: it has no file to read again",
            buffer.name()
        )),
    }
}

/// The bytes of text a handover carries apart from kills, or an error naming the largest
/// buffer if it is more than a restart carries, or naming a buffer that cannot be handed
/// over.
fn carried_bytes(editor: &Editor) -> Result<usize, String> {
    let mut total: usize = editor
        .histories()
        .flat_map(|(_, answers)| answers.map(str::len))
        .sum();
    let mut largest: Option<&Buffer> = None;
    for (_, buffer) in editor.buffers() {
        if let Handed::Carried(_) = handed(buffer)? {
            total += buffer.text().len();
            if largest.is_none_or(|largest| largest.text().len() < buffer.text().len()) {
                largest = Some(buffer);
            }
        }
    }
    match largest {
        Some(largest) if total > MAX_CARRIED => Err(format!(
            "cannot hand over: the unsaved text takes {total} bytes, more than the \
             {MAX_CARRIED} a restart carries; save {}",
            largest.name()
        )),
        _ => Ok(total),
    }
}

/// The histories, each as one item of its answers, the newest first.
fn histories_state(editor: &Editor, items: &mut Vec<Item>) -> Vec<Value> {
    editor
        .histories()
        .map(|(name, entries)| {
            let entries: Vec<&str> = entries.collect();
            items.push(Item(vec![Rope::from(entries.concat().as_str())]));
            json!({
                "name": name,
                "item": items.len() - 1,
                "lengths": entries.iter().map(|entry| entry.len()).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// The kill entries a handover carries, the newest first: none that came from a buffer that
/// may hold secrets or from the clipboard (ADR 0074), and only as many of the newest as fit
/// in `room` bytes and the bound on pieces. An O(1) snapshot of each piece's parts.
fn kills_state(editor: &Editor, mut room: usize, items: &mut Vec<Item>) -> Vec<Value> {
    let mut pieces_left = MAX_KILL_PIECES;
    let mut kills = Vec::new();
    let carried = editor
        .kill_ring()
        .iter()
        .filter(|entry| !entry.is_secret() && !entry.came_from_clipboard() && !entry.is_empty());
    for entry in carried {
        let len: usize = entry.pieces().iter().map(|piece| piece.len()).sum();
        let pieces = entry.pieces().len();
        if len > room || pieces > pieces_left {
            log::info!(
                "oso dev: kills older than the {} carried do not fit in a restart",
                kills.len()
            );
            break;
        }
        room -= len;
        pieces_left -= pieces;
        kills.push(kill_state(entry, items));
    }
    kills
}

fn kill_state(entry: &KillEntry, items: &mut Vec<Item>) -> Value {
    let parts = entry
        .pieces()
        .iter()
        .flat_map(|piece| piece.parts().iter().cloned())
        .collect();
    items.push(Item(parts));
    json!({
        "item": items.len() - 1,
        "pieces": entry.pieces().iter().map(|piece| piece.len()).collect::<Vec<_>>(),
    })
}

/// The result of `check-handover`: the instance's generation, and whether it holds
/// text no file does, or why its text would not survive a restart. These are the
/// buffers `export` would refuse, without building a state, so that the supervisor can
/// ask before it opens a new window. One that holds such text can hand over, but must
/// not be closed (ADR 0055).
pub fn check(editor: &Editor) -> Reply {
    if shows_judged_secret(editor) {
        return Err(SECRET_SHOWN.into());
    }
    carried_bytes(editor)?;
    let unsaved = editor
        .buffers()
        .any(|(_, buffer)| matches!(handed(buffer), Ok(Handed::Carried(_))));
    if editor.preedit().is_some() {
        return Err(COMPOSING.into());
    }
    if editor.prompt().is_some() {
        return Err(PROMPTING.into());
    }
    if editor.isearch().is_some() {
        return Err(SEARCHING.into());
    }
    Ok(json!({ "generation": generation(editor), "unsaved": unsaved }))
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

/// Names what an instance holds that a file does not: which buffers exist, the
/// content of each modified one, and the kill ring's newest entry and size. It is
/// not a counter, as undoing an edit brings the old value back, and only the
/// instance that made it compares it.
///
/// A buffer that is not modified counts only by its id, since each piece of a
/// load gets a new content id and a large file may still be arriving. A save of a
/// modified buffer changes it, as the buffer stops being modified. Marks, cursors
/// and histories do not count: they change all the time in a window in use, and
/// put no text at stake (ADR 0090).
pub fn generation(editor: &Editor) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (id, buffer) in editor.buffers() {
        id.hash(&mut hasher);
        buffer
            .is_modified()
            .then(|| buffer.content_id())
            .hash(&mut hasher);
    }
    let ring = editor.kill_ring();
    ring.len().hash(&mut hasher);
    ring.get(0)
        .map(|newest| (newest.id(), newest.len()))
        .hash(&mut hasher);
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

/// Positions waiting for their file to load, since a cursor or a mark cannot sit in
/// text that has not arrived. The file may also have changed since the export, so
/// they are moved to the nearest place that exists. Carried text is all there, so
/// its positions apply at once.
#[derive(Debug)]
pub struct Restore {
    pub buffer: BufferId,
    pub selections: SelectionSet,
    pub scroll: Scroll,
    /// Whether the scroll's start is a logical line, from a state before version 3, which
    /// kept the top as a row.
    pub top_is_row: bool,
}

impl Restore {
    /// Moves the window to the positions if the buffer holds them by now, or, if the
    /// window has gone to another buffer meanwhile, the place it keeps for this one, which
    /// showing it again gives back (ADR 0090). Returns whether this is over: restored, or the
    /// window keeps no place for the buffer.
    pub fn try_apply(&self, editor: &mut Editor) -> bool {
        let window = editor.tiled().0;
        let shown = window.buffer() == self.buffer;
        let kept = window.places().any(|(buffer, _)| buffer == self.buffer);
        let Some(buffer) = editor.buffer(self.buffer).filter(|_| shown || kept) else {
            return true;
        };
        let selections = self.selections.iter();
        let marks = selections.flat_map(|s| [Some(s.cursor), s.mark]).flatten();
        let needed = marks
            .chain(self.selections.mark_ring().iter())
            .max()
            .unwrap_or(0);
        if buffer.loading().is_some() && buffer.text().len() < needed {
            return false;
        }
        let display = buffer.display();
        let selections = self.clipped(&display);
        let mut scroll = self.scroll;
        if self.top_is_row {
            let row = scroll.start.min(display.row_count().saturating_sub(1));
            scroll.start = oso_core::motion::line_start(&display, row);
        }
        if shown {
            editor.restore_positions(selections, scroll);
        } else {
            editor.restore_kept_place(self.buffer, selections, scroll);
        }
        true
    }

    /// The positions, each moved to the start of the display cluster it is in, or the end.
    fn clipped(&self, display: &DisplaySnapshot) -> SelectionSet {
        let clip = |offset: usize| {
            let point: DisplayPoint = display.offset_to_display_point(offset);
            display.display_point_to_offset(point, Bias::Left)
        };
        let selections = self
            .selections
            .iter()
            .map(|selection| Selection {
                cursor: clip(selection.cursor),
                mark: selection.mark.map(clip),
                goal: selection.goal,
            })
            .collect();
        let mut set = SelectionSet::from_parts(selections, self.selections.primary_index())
            .unwrap_or_default();
        set.set_mark_active(self.selections.mark_active());
        let ring = MarkRing::from_newest(self.selections.mark_ring().iter().map(clip));
        set.with_ring(ring)
    }
}

/// [`export`]'s state alone, for tests.
#[cfg(test)]
fn export_value(
    editor: &Editor,
    pending: &[KeyPress],
    frame: Option<Frame>,
    restore: Option<&Restore>,
) -> Reply {
    export(editor, pending, frame, restore, 1).map(|(state, _)| state)
}

#[cfg(test)]
mod internal_tests;
/// The theme's name, if the state has one, as the built-in themes are named: up to 32
/// lower-case letters, digits and `-`. A state from before themes has none.
fn parse_theme(state: &Map<String, Value>) -> Result<Option<String>, String> {
    let named = |name: &&str| {
        (1..=32).contains(&name.len())
            && name
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'))
    };
    match state.get("theme") {
        None | Some(Value::Null) => Ok(None),
        Some(theme) => theme
            .as_str()
            .filter(named)
            .map(|name| Some(name.to_owned()))
            .ok_or_else(|| "the theme is not a theme's name".into()),
    }
}

#[cfg(test)]
mod tests;
