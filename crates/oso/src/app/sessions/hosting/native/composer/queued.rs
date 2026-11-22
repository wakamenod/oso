//! A native session's queue in its transcript (ADR 0165, step 2): its rows, grey at the
//! transcript's end after its rows, which new rows go in above, and what the keys on a queued
//! row do: `RET` takes the message back, `e` edits it in its place, `d` drops it and
//! `M-<up>`/`M-<down>` move it, point with it.
//!
//! Must not depend on: key presses or how a frame is drawn; the composer's keys ask.

use oso_core::buffer::Replacement;
use oso_core::editor::NativeAction;

use crate::app::App;
use crate::sessions::native::queue::Move;
use crate::sessions::native::render::{Face, queue_rows};
use crate::sessions::worker::SessionKey;

impl App {
    /// Draws native session `key`'s queue as grey rows at its transcript's end, when they
    /// changed, and says how many wait in its modeline.
    pub(in crate::app::sessions::hosting::native) fn draw_queue(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let rows = queue_rows(&native.composing.queue);
        if rows != native.composing.rows {
            let start = native.placed.end();
            let range = start..start + native.composing.rows.len();
            let runs = [(rows.len(), Some(Face::Dim.highlight()))];
            self.editor
                .replace_native_rows(native.buffer, range, &rows, &runs);
            native.composing.rows = rows;
        }
        self.write_native_note(key);
    }

    /// The queued message whose row is at `offset` of native session `key`'s transcript; the
    /// queue's heading names the first.
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
