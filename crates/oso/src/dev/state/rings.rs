//! The kill ring and the global mark ring in `get-state` (ADR 0074, ADR 0190): what they hold
//! that may be shown, never a secret's text or a place in a buffer that may hold secrets.
//!
//! Must not depend on: `app`, winit or sockets.

use oso_core::editor::Editor;
use serde_json::{Value, json};

/// The most pieces of the newest entry `get-state` shows, and the most bytes of each.
const SHOWN_PIECES: usize = 16;
pub(super) const SHOWN_PIECE_BYTES: usize = 1024;

/// The `kill_ring` field of `get-state`: how many entries there are, and the newest one's
/// pieces, cut short. Secret entries and text from other apps' copies are left out as if
/// absent, and the whole field is null while a buffer that may hold secrets is shown
/// (ADR 0074).
pub(super) fn kill_ring_state(editor: &Editor) -> Value {
    if editor.shows_secret() {
        return Value::Null;
    }
    let mut ordinary = editor
        .kill_ring()
        .iter()
        .filter(|entry| !entry.is_secret() && !entry.came_from_clipboard());
    let newest = ordinary.next().map(|entry| {
        let pieces = entry.pieces();
        let mut truncated = pieces.len() > SHOWN_PIECES;
        let shown: Vec<String> = pieces
            .iter()
            .take(SHOWN_PIECES)
            .map(|piece| {
                truncated |= piece.len() > SHOWN_PIECE_BYTES;
                piece_start(piece)
            })
            .collect();
        json!({ "pieces": shown, "truncated": truncated })
    });
    json!({ "entries": usize::from(newest.is_some()) + ordinary.count(), "newest": newest })
}

/// The first [`SHOWN_PIECE_BYTES`] of `piece`, cut at a character's start.
fn piece_start(piece: &oso_core::kill::Piece) -> String {
    let mut text = String::new();
    for chunk in piece.chunks() {
        let room = SHOWN_PIECE_BYTES - text.len();
        if chunk.len() <= room {
            text.push_str(chunk);
            continue;
        }
        let mut end = room;
        while !chunk.is_char_boundary(end) {
            end -= 1;
        }
        text.push_str(&chunk[..end]);
        break;
    }
    text
}

/// The `global_marks` field of `get-state`: the global mark ring's places, the newest first,
/// each its buffer and offset, those in a buffer that may hold secrets left out as if absent,
/// as an offset tells how long words are there (ADR 0058, ADR 0190).
pub(super) fn global_marks_state(editor: &Editor) -> Value {
    editor
        .global_marks()
        .iter()
        .filter(|(buffer, _)| {
            editor
                .buffer(*buffer)
                .is_some_and(|shown| !shown.is_secret())
        })
        .map(|(buffer, offset)| json!({ "buffer": buffer.get(), "offset": offset }))
        .collect()
}
