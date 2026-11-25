//! How a native session's transcript is drawn (ADR 0165): each row that changed where it is, and
//! what stands after the rows, put a part at a time: the messages being sent, the queue's and the
//! block of the call that waits, one tail (ADR 0182).
//!
//! Must not depend on: key presses or how a frame is drawn; the rows and the tail say what stands
//! where.

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;

use super::{Native, sending};
use crate::sessions::native::render;
use crate::sessions::native::rows::RowChange;
use crate::sessions::native::tail::{Part, Run};

/// Draws each of `changes` in the session's buffer, in their rows' order, where `Placed` says
/// each row is. Whether the messages being sent were drawn too, as without a row that changed.
pub(super) fn draw(
    editor: &mut Editor,
    native: &mut Native,
    changes: impl IntoIterator<Item = RowChange>,
) -> bool {
    let mut changes = changes.into_iter().peekable();
    if changes.peek().is_some() {
        sending::before_rows(editor, native);
    }
    for change in changes {
        let at = change.row();
        let Some(row) = native.rows.rows().get(at) else {
            continue;
        };
        let Some(text) = editor.buffer(native.buffer).map(Buffer::text) else {
            return false;
        };
        let range = native.placed.range(change, |start, lines| {
            let first = text.offset_to_point(start).row;
            text.line_start(first + lines)
        });
        let Some(range) = range else {
            continue;
        };
        let rendered = match change {
            RowChange::Grew { at, .. } => render::tail(row, at, native.rows.width()),
            RowChange::Added(_) | RowChange::Changed(_) => {
                render::render(row, native.rows.directory(), native.rows.width())
            }
        };
        let grew = matches!(change, RowChange::Grew { .. });
        if !grew {
            if rendered.calls.is_empty() {
                native.calls.remove(&at);
            } else {
                native.calls.insert(at, rendered.calls.clone());
            }
        }
        (native.widths).drawn(at, row, &rendered, grew, native.rows.width());
        let runs = rendered.runs();
        // Drawn again: a whole reply is highlighted again (part 2g).
        native.markdown.drawn(at);
        editor.replace_native_rows(native.buffer, range.clone(), &rendered.text, &runs);
        native.placed.put(at, range, rendered.text.len());
        debug_assert_eq!(
            editor.buffer(native.buffer).map(|b| b.text().len()),
            Some(native.placed.end() + native.tail.len())
        );
    }
    sending::sync(editor, native)
}

/// Puts `text`, in `runs`, as `part` of what stands after the session's rows. Whether it changed.
pub(super) fn put_tail(
    editor: &mut Editor,
    native: &mut Native,
    part: Part,
    text: String,
    runs: Vec<Run>,
) -> bool {
    let start = native.placed.end();
    let Some(place) = native.tail.put(part, text, runs) else {
        return false;
    };
    let (text, runs) = native.tail.text(part);
    let range = start + place.start..start + place.end;
    editor.replace_native_rows(native.buffer, range, text, runs);
    debug_assert_eq!(
        editor.buffer(native.buffer).map(|b| b.text().len()),
        Some(start + native.tail.len())
    );
    true
}
