//! A native session's queue in its transcript (ADR 0165, step 2): its rows, grey at the
//! transcript's end after its rows, which new rows go in above, and what the keys on a queued
//! row do: `RET` takes the message back, `e` edits it in its place, `d` drops it and
//! `M-<up>`/`M-<down>` move it, point with it.
//!
//! Must not depend on: key presses or how a frame is drawn; the composer's keys ask.

use oso_core::buffer::{Buffer, BufferId, Replacement};
use oso_core::editor::{Editor, NativeAction};
use oso_core::window::WindowId;

use crate::app::App;
use crate::sessions::native::queue::Move;
use crate::sessions::native::render::{Face, queue_rows};
use crate::sessions::native::suggest::suggestion_row;
use crate::sessions::worker::SessionKey;

/// The windows whose point is on a row of those from `start`, the queue's and the suggestion's,
/// in `buffer`, with the row's number: below the heading and before the end, which stays the
/// end.
fn windows_on_rows(editor: &Editor, buffer: BufferId, start: usize) -> Vec<(WindowId, usize)> {
    let Some(text) = editor.buffer(buffer).map(Buffer::text) else {
        return Vec::new();
    };
    let first = text.offset_to_point(start).row;
    (editor.windows())
        .filter(|window| window.buffer() == buffer)
        .filter_map(|window| {
            let cursor = window.selections.primary().cursor;
            let row = text.offset_to_point(cursor).row.checked_sub(first)?;
            (row > 0 && cursor < text.len()).then(|| (window.id(), row))
        })
        .collect()
}

/// Puts the point of `window`, which was on a row of the queue in `buffer`, whose rows are from
/// `start`, on its message's row, `at`, from 0, or at the end if the message is gone, so that a
/// key never acts on another.
fn stay_on_row(
    editor: &mut Editor,
    buffer: BufferId,
    start: usize,
    window: WindowId,
    at: Option<usize>,
) {
    let Some(text) = editor.buffer(buffer).map(Buffer::text) else {
        return;
    };
    let offset = match at {
        Some(at) => text.line_start(text.offset_to_point(start).row + 1 + at),
        None => text.len(),
    };
    if let Some(shown) = editor.window_by_id(window) {
        let (mut selections, scroll) = (shown.selections.clone(), shown.scroll());
        selections.move_primary(offset);
        editor.restore_positions_in(window, selections, scroll);
    }
}

impl App {
    /// Draws native session `key`'s queue as grey rows at its transcript's end, and its
    /// prompt suggestion's after them (part 3f), when they changed, and says how many wait in
    /// its modeline.
    pub(in crate::app::sessions::hosting::native) fn draw_queue(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let mut rows = queue_rows(&native.composing.queue);
        if let Some(text) = &native.composing.suggestion {
            rows.push_str(&suggestion_row(text));
        }
        if rows != native.composing.rows {
            let start = native.placed.end();
            let range = start..start + native.composing.rows.len();
            let runs = [(rows.len(), Some(Face::Dim.highlight()))];
            let now: Vec<u64> = (native.composing.queue.waiting())
                .map(|queued| queued.id)
                .collect();
            let drawn = std::mem::replace(&mut native.composing.drawn, now);
            let on = windows_on_rows(&self.editor, native.buffer, start);
            self.editor
                .replace_native_rows(native.buffer, range, &rows, &runs);
            native.composing.rows = rows;
            for (window, row) in on {
                let id = row.checked_sub(1).and_then(|at| drawn.get(at));
                let at = native
                    .composing
                    .drawn
                    .iter()
                    .position(|now| Some(now) == id);
                stay_on_row(&mut self.editor, native.buffer, start, window, at);
            }
        }
        self.write_native_note(key);
    }

    /// The queued message whose row is at `offset` of native session `key`'s transcript; the
    /// queue's heading names the first, and a row after the queue's is the suggestion's.
    pub(super) fn queued_at(&self, key: SessionKey, offset: usize) -> Option<u64> {
        let native = self.hosting.natives.map.get(&key)?;
        let start = native.placed.end();
        if offset < start || native.composing.queue.is_empty() {
            return None;
        }
        let text = self.editor.buffer(native.buffer)?.text();
        let line = text.offset_to_point(offset).row - text.offset_to_point(start).row;
        let at = line.saturating_sub(1);
        (native.composing.queue.waiting())
            .nth(at)
            .map(|queued| queued.id)
    }

    /// `C-c C-q`: point to the queue's row `at`, from 0, in the transcript.
    pub(super) fn queue_go(&mut self, key: SessionKey, at: usize) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        if native.composing.queue.is_empty() {
            return self.editor.message("Nothing is queued");
        }
        let (transcript, start) = (native.buffer, native.placed.end());
        if self.editor.current().0.buffer() != transcript {
            self.composer_go(key, false);
        }
        if self.editor.current().0.buffer() != transcript {
            return;
        }
        let Some(row) = self.editor.buffer(transcript).map(|buffer| {
            let text = buffer.text();
            text.line_start(text.offset_to_point(start).row + 1 + at)
        }) else {
            return;
        };
        self.editor.current_mut().0.selections.move_primary(row);
    }

    /// `RET` on a queued row: its message goes back into the composer, above what is there.
    pub(super) fn queued_take_back(&mut self, key: SessionKey, id: u64) {
        let empty = self.composer_empty(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let Some(mut taken) = native.composing.queue.take_back(id) else {
            return;
        };
        if !empty {
            taken.push_str("\n\n");
        }
        if let Some(composer) = self.composer(key) {
            let edit = Replacement {
                range: 0..0,
                text: &taken,
            };
            let _ = self.editor.apply_edits(composer, &[edit]);
        }
        self.draw_queue(key);
        self.composer_go(key, true);
    }

    /// `e`, `d`, `M-<up>` and `M-<down>` on queued message `id`.
    pub(super) fn queued_act(&mut self, key: SessionKey, id: u64, action: NativeAction) {
        let empty = self.composer_empty(key);
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let queue = &mut native.composing.queue;
        match action {
            NativeAction::EditQueued if !empty => self
                .editor
                .message("The composer holds a message: send or clear it first"),
            NativeAction::EditQueued => {
                let Some(text) = queue.edit(id) else {
                    return;
                };
                self.set_composer_text(key, &text);
                self.draw_queue(key);
                self.composer_go(key, true);
            }
            NativeAction::DropQueued => {
                queue.drop_one(id);
                self.draw_queue(key);
            }
            NativeAction::MoveQueued(count) => {
                let way = if count < 0 { Move::Up } else { Move::Down };
                let mut place = None;
                for _ in 0..count.unsigned_abs() {
                    match queue.shift(id, way) {
                        Some(at) => place = Some(at),
                        None => break,
                    }
                }
                self.draw_queue(key);
                // Point stays on the message moved.
                if let Some(at) = place {
                    self.queue_go(key, at);
                }
            }
            _ => {}
        }
    }
}
