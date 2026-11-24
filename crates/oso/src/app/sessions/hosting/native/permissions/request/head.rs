//! The head of a native session's call's block, shown by a window that follows its transcript, where
//! the block is taller than the window (ADR 0182, "Point" 7).
//!
//! Must not depend on: key presses or how a frame is drawn; the host says when a block came, an event
//! ended, or it goes.
//!
//! - Once a call and a window: the first time a window is looked at while the call waits, with no
//!   region or search in it.
//! - Point goes to the block's first row, so that the window follows no more. As the block goes, a
//!   window whose first row is in it, and point then, is at the transcript's end.
//! - Nothing of it counts as read: what a window shows is what is read.

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::selection::MarkActive;
use oso_core::window::{Scroll, Window, WindowId};

use crate::app::App;
use crate::app::sessions::hosting::native::Native;
use crate::sessions::native::tail::Part;
use crate::sessions::worker::SessionKey;

/// The windows looked at for a block's head.
#[derive(Debug, Default)]
pub(super) struct Heads {
    /// Each, and whether oso showed the head in it.
    seen: Vec<(WindowId, bool)>,
}

/// The first byte of `native`'s block: its title, past the row between it and what is above.
pub(super) fn first(native: &Native) -> usize {
    native.part_place(Part::Title).start + 1
}

/// Puts point of window `id` at `offset` alone, the view at `top` and then as little from there as
/// shows it.
pub(super) fn put(editor: &mut Editor, id: WindowId, offset: usize, top: Scroll) {
    let Some(window) = editor.window_by_id(id) else {
        return;
    };
    let mut selections = window.selections.clone();
    selections.keep_primary();
    selections.move_primary(offset);
    editor.restore_positions_in(id, selections, top);
}

/// Whether window `id` may be taken to a block's head: no region is marked in it, and no search runs
/// there. Looked at again after each event until it may.
fn free(editor: &Editor, id: WindowId) -> bool {
    let searched = editor.tiled().0.id() == id
        && (editor.isearch().is_some() || editor.replace().is_some() || editor.jump().is_some());
    let marked = |window: &Window| window.selections.mark_active() != MarkActive::Inactive;
    !searched && !editor.window_by_id(id).is_some_and(marked)
}

/// Puts window `id` at `native`'s block's head, if it follows its transcript, as its point is at the
/// end, and the block is taller than it. Whether it did.
pub(super) fn show(editor: &mut Editor, native: &Native, id: WindowId) -> bool {
    let (Some(window), Some(buffer)) = (editor.window_by_id(id), editor.buffer(native.buffer))
    else {
        return false;
    };
    let end = buffer.text().len();
    let follows = window.selections.len() == 1 && window.selections.primary().cursor == end;
    let start = first(native);
    if !follows || window.scroll().start <= start {
        return false;
    }
    put(editor, id, start, Scroll { start, left: 0 });
    true
}

/// The windows whose first row is in `native`'s block, and point then, which is in view.
pub(super) fn in_block(editor: &Editor, native: &Native) -> Vec<WindowId> {
    let start = native.part_place(Part::Title).start;
    let inside =
        |window: &&Window| window.buffer() == native.buffer && window.scroll().start >= start;
    (editor.windows().filter(inside)).map(Window::id).collect()
}

/// Puts each of `windows`, which show `transcript`, at its end, its last row at the window's bottom.
pub(super) fn to_end(editor: &mut Editor, transcript: BufferId, windows: &[WindowId]) {
    let Some(end) = editor.buffer(transcript).map(|buffer| buffer.text().len()) else {
        return;
    };
    if !windows.is_empty() {
        editor.refill_wraps();
    }
    for &id in windows {
        // From above it, so that the end comes in at the bottom.
        put(editor, id, end, Scroll::default());
    }
}

impl App {
    /// Shows the head of session `key`'s call's block in each window not looked at before.
    pub(super) fn show_heads(&mut self, key: SessionKey) {
        let natives = &mut self.hosting.natives;
        let Some(native) = natives.map.get(&key) else {
            return;
        };
        let Some(drawn) = natives.permissions.requests.drawn(key) else {
            return;
        };
        let heads = &mut drawn.heads.seen;
        let new: Vec<WindowId> = (self.editor.windows())
            .filter(|window| window.buffer() == native.buffer)
            .map(Window::id)
            .filter(|id| heads.iter().all(|(seen, _)| seen != id))
            .filter(|id| (self.editor.regions(*id)).is_some_and(|regions| regions.text.rows > 0))
            .filter(|id| free(&self.editor, *id))
            .collect();
        if new.is_empty() {
            return;
        }
        let editor = &mut self.editor;
        heads.retain(|(id, _)| editor.window_by_id(*id).is_some());
        // One budget for all of them, as a command's.
        editor.refill_wraps();
        for id in new {
            heads.push((id, show(editor, native, id)));
        }
    }

    /// Where oso put point in window `id` as it showed the head of session `key`'s call's block, as
    /// that is now.
    pub(super) fn head_of(&self, key: SessionKey, id: WindowId) -> Option<usize> {
        let drawn = &self.hosting.natives.permissions.requests.drawn;
        let drawn = drawn.iter().find(|drawn| drawn.key == key)?;
        let shown = (drawn.heads.seen.iter()).any(|&(window, shown)| shown && window == id);
        let native = self.hosting.natives.map.get(&key).filter(|_| shown)?;
        Some(first(native))
    }
}

#[cfg(test)]
#[path = "head_tests.rs"]
mod tests;
