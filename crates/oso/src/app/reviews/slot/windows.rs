//! Where the review layout's windows are (ADR 0160): the review's window and the slot, found by
//! name, the slot made again at the review's right when the person deleted it and fitted to two
//! fifths of its row, and what a restart carried of them.
//!
//! Must not depend on: key presses or how a frame is drawn; this reads and changes the editor's
//! windows alone.

use oso_core::buffer::BufferId;
use oso_core::editor::{Editor, ReviewId};
use oso_core::frame::{Way, WindowRegions};
use oso_core::window::WindowId;

use super::{SLOT_FIFTHS, first_slot_buffer, owner};
use crate::sessions::spaces::kept::{self, Kept, ReviewKept};

/// The window of the frame showing the layout's review: its own while it shows a review, else
/// the first in the tree's order showing the layout's review, which becomes its own.
pub(super) fn layout_window(editor: &Editor, kept: &mut [Kept]) -> Option<WindowId> {
    let laid = kept::review(kept)?;
    let (review, own) = (laid.review, laid.window);
    if shows_review(editor, own) {
        return Some(own);
    }
    let found = (editor.frame().windows().iter())
        .map(|(id, _)| *id)
        .filter(|id| in_tree(editor, *id))
        .find(|id| {
            (editor.window_by_id(*id))
                .is_some_and(|window| owner(editor, window.buffer()) == Some(review))
        })?;
    if let Some(laid) = kept::review_mut(kept) {
        laid.window = found;
    }
    Some(found)
}

/// Selects the slot for a jump's source while the frame is the layout, made again at the
/// review's right when the person deleted it. Returns whether it did: outside the layout, the
/// jump keeps its own rule (ADR 0122, step 3).
pub(super) fn jump_slot(editor: &mut Editor, kept: &mut [Kept]) -> bool {
    let from = editor.window().id();
    let Some(slot) = slot(editor, kept, from) else {
        return false;
    };
    if let Some(laid) = kept::review_mut(kept) {
        laid.from = Some(from);
    }
    editor.select_window(slot);
    true
}

/// Shows `buffer` in the slot while the frame is the review layout, selecting it with
/// `select`. Returns whether it did.
pub(super) fn show_in_slot(
    editor: &mut Editor,
    kept: &mut [Kept],
    buffer: BufferId,
    select: bool,
) -> bool {
    let from = editor.window().id();
    let Some(slot) = slot(editor, kept, from) else {
        return false;
    };
    editor.show_in(slot, buffer);
    if select {
        if shows_review(editor, from)
            && let Some(laid) = kept::review_mut(kept)
        {
            laid.from = Some(from);
        }
        editor.select_window(slot);
    }
    true
}

/// Whether the selected window is the layout's slot, while the frame is the layout.
pub(super) fn in_slot(editor: &Editor, kept: &[Kept]) -> bool {
    kept::review_in_frame(kept)
        && kept::review(kept).and_then(|laid| laid.slot) == Some(editor.window().id())
}

/// The layout's slot while the frame is the layout, made again at the right of the review's
/// window when it is gone, and never the window `from` the person is in. None outside the
/// layout, or where no slot fits.
pub(super) fn slot(editor: &mut Editor, kept: &mut [Kept], from: WindowId) -> Option<WindowId> {
    if !kept::review_in_frame(kept) {
        return None;
    }
    let laid = kept::review(kept)?;
    if let Some(slot) = (laid.slot).filter(|slot| *slot != from && in_tree(editor, *slot)) {
        return Some(slot);
    }
    // Split off the review's window the person is in, else the layout's.
    let base = if shows_review(editor, from) {
        from
    } else {
        layout_window(editor, kept)?
    };
    let selected = editor.window().id();
    editor.select_window(base);
    // What it shows until a jump's file comes: no copy of the review.
    let first = first_slot_buffer(editor);
    let made = editor.split_window(Way::Across, false).ok();
    if let Some(slot) = made {
        editor.show_in(slot, first);
        fit_slot(editor, slot);
    }
    if editor.window_by_id(selected).is_some() {
        editor.select_window(selected);
    }
    let made = made?;
    if let Some(laid) = kept::review_mut(kept) {
        laid.slot = Some(made);
    }
    Some(made)
}

/// A restart carried the layout of the review `review` shown in `window`, with its slot in
/// `slot`: the frame is the layout again, with nothing to restore.
pub(super) fn carried(
    editor: &Editor,
    kept: &mut Vec<Kept>,
    review: ReviewId,
    window: WindowId,
    slot: WindowId,
) {
    if kept::review(kept).is_some() || !in_tree(editor, slot) || !in_tree(editor, window) {
        return;
    }
    let pushed = kept::push_review(
        kept,
        ReviewKept {
            before: None,
            review,
            window,
            slot: Some(slot),
            from: None,
        },
    );
    debug_assert!(pushed, "one layout a tree");
}

/// The layout's slot while the frame is the layout of `review`.
pub(super) fn slot_of(kept: &[Kept], review: ReviewId) -> Option<WindowId> {
    let laid = kept::review(kept).filter(|laid| laid.review == review)?;
    kept::review_in_frame(kept).then_some(laid.slot).flatten()
}

/// Whether window `id` is in the frame's tree and shows a review's buffer.
pub(super) fn shows_review(editor: &Editor, id: WindowId) -> bool {
    in_tree(editor, id)
        && (editor.window_by_id(id)).is_some_and(|window| owner(editor, window.buffer()).is_some())
}

/// Whether window `id` is one of the frame's tree, laid out with room: not the side window.
pub(super) fn in_tree(editor: &Editor, id: WindowId) -> bool {
    Some(id) != editor.side_window()
        && (editor.frame().windows().iter())
            .any(|(window, regions)| *window == id && regions.modeline.rows > 0)
}

/// Makes the window `slot` two fifths of the row it is in, by the windows beside it, leaving
/// whichever window is selected selected.
pub(super) fn fit_slot(editor: &mut Editor, slot: WindowId) {
    let side = editor.side_window();
    let windows = editor.frame().windows();
    let Some(regions) = (windows.iter()).find_map(|(id, r)| (*id == slot).then_some(r)) else {
        return;
    };
    let (top, bottom) = rows(regions);
    let (mut left, mut right) = span(regions);
    let own = right - left;
    for (_, other) in windows.iter().filter(|(id, _)| Some(*id) != side) {
        let (other_top, other_bottom) = rows(other);
        if other_top < bottom && top < other_bottom && other.modeline.rows > 0 {
            let (start, end) = span(other);
            left = left.min(start);
            right = right.max(end);
        }
    }
    let want = ((right - left) * SLOT_FIFTHS + 2) / 5;
    let (Ok(want), Ok(own)) = (isize::try_from(want), isize::try_from(own)) else {
        return;
    };
    let selected = editor.window().id();
    editor.select_window(slot);
    let _ = editor.resize_window(Way::Across, want - own);
    editor.select_window(selected);
}

/// The columns a window takes, its divider's too: from its left edge to its right.
pub(super) fn span(regions: &WindowRegions) -> (usize, usize) {
    let left = regions.modeline.column;
    let divider = regions.divider.map_or(0, |divider| divider.columns);
    (left, left + regions.modeline.columns + divider)
}

/// The rows a window takes, its modeline's too.
pub(super) fn rows(regions: &WindowRegions) -> (usize, usize) {
    (
        regions.text.row,
        regions.modeline.row + regions.modeline.rows,
    )
}
