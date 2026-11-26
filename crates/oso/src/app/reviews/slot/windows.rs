//! Where the review layout's windows are (ADR 0160): the review's window and the slot, found by
//! name, the slot made again at the review's right when the person deleted it and fitted to two
//! fifths of its row, and what a restart carried of them. While the slot shows something other
//! than the session `review-ask` shows, the session goes to a window below it, the slot's lower
//! half, which is lent the terminal's size while no window showing it owns that (ADR 0169).
//!
//! Must not depend on: key presses or how a frame is drawn; this reads and changes the editor's
//! windows alone.

use oso_core::buffer::BufferId;
use oso_core::editor::{Editor, ReviewId};
use oso_core::frame::{Way, WindowRegions};
use oso_core::window::{Window, WindowId};

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
    keep_asked_below(editor, kept, slot);
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
    keep_asked_below(editor, kept, slot);
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

/// Shows the session's buffer `buffer` for `review-ask` while the frame is the review layout,
/// not selected: in the window below the slot while there is one; else in the slot while it shows
/// that session or `*Messages*`; else in a window split off below the slot, its lower half, so
/// that what the slot shows stays on screen (ADR 0169). A slot too small to split shows it in
/// place. Returns whether it showed it.
pub(super) fn show_asked(editor: &mut Editor, kept: &mut [Kept], buffer: BufferId) -> bool {
    let from = editor.window().id();
    if !kept::review_in_frame(kept) {
        return false;
    }
    let (window, below) = match ask_window(editor, kept).filter(|ask| *ask != from) {
        Some(ask) => (ask, Some(ask)),
        None => {
            let Some(slot) = slot(editor, kept, from) else {
                return false;
            };
            let shows = editor.window_by_id(slot).map(Window::buffer);
            let filler = shows == Some(buffer) || shows == Some(editor.messages_buffer());
            match (!filler).then(|| split_below(editor, slot)).flatten() {
                Some(below) => (below, Some(below)),
                None => (slot, None),
            }
        }
    };
    editor.show_in(window, buffer);
    if let Some(laid) = kept::review_mut(kept) {
        laid.ask = below;
        laid.asked = Some(buffer);
    }
    true
}

/// Before the slot `slot` shows something else: while it shows the session `review-ask` showed
/// last, with no window below it showing that, the slot splits and the session goes below, so
/// that a jump or a message takes the slot's upper half alone (ADR 0169).
fn keep_asked_below(editor: &mut Editor, kept: &mut [Kept], slot: WindowId) {
    let Some(asked) = kept::review(kept).and_then(|laid| laid.asked) else {
        return;
    };
    let fills = (editor.window_by_id(slot)).is_some_and(|window| window.buffer() == asked);
    if !fills || ask_window(editor, kept).is_some() {
        return;
    }
    if let Some(below) = split_below(editor, slot) {
        editor.show_in(below, asked);
        if let Some(laid) = kept::review_mut(kept) {
            laid.ask = Some(below);
        }
    }
}

/// The window below the slot that `review-ask` shows its session in, while it is in the tree,
/// is not the slot and shows no review.
pub(super) fn ask_window(editor: &Editor, kept: &[Kept]) -> Option<WindowId> {
    let laid = kept::review(kept)?;
    (laid.ask).filter(|ask| {
        Some(*ask) != laid.slot && in_tree(editor, *ask) && !shows_review(editor, *ask)
    })
}

/// The ask window and the terminal it shows, while the frame is the layout and no window
/// showing that terminal owns its size, which `owned` says (None for a buffer that is no
/// terminal): the ask window is lent the size then, so that the session draws to the half it
/// has (ADR 0169, amending ADR 0120's rule that the window that selected a terminal last owns
/// its size).
pub(super) fn ask_takes_size(
    editor: &Editor,
    kept: &[Kept],
    owned: impl Fn(BufferId) -> Option<bool>,
) -> Option<(BufferId, WindowId)> {
    if !kept::review_in_frame(kept) {
        return None;
    }
    let ask = ask_window(editor, kept)?;
    let buffer = editor.window_by_id(ask)?.buffer();
    (!owned(buffer)?).then_some((buffer, ask))
}

/// Splits `window` into two stacked halves, the new one below, showing what it shows, leaving
/// the selected window selected. None where either half would be too small.
fn split_below(editor: &mut Editor, window: WindowId) -> Option<WindowId> {
    let selected = editor.window().id();
    editor.select_window(window);
    let made = editor.split_window(Way::Down, false).ok();
    if editor.window_by_id(selected).is_some() {
        editor.select_window(selected);
    }
    made
}

/// Whether the selected window is the layout's slot, while the frame is the layout.
pub(super) fn in_slot(editor: &Editor, kept: &[Kept]) -> bool {
    kept::review_in_frame(kept)
        && kept::review(kept).and_then(|laid| laid.slot) == Some(editor.window().id())
}

/// The layout's slot while the frame is the layout, made again at the right of the review's
/// window when it is gone, and never the window `from` the person is in. None outside the
/// layout, or where no slot fits. With the slot gone, or the window the person is in, and the
/// window below it still there, that window splits: its upper half becomes the slot, the
/// session staying below (ADR 0169).
pub(super) fn slot(editor: &mut Editor, kept: &mut [Kept], from: WindowId) -> Option<WindowId> {
    if !kept::review_in_frame(kept) {
        return None;
    }
    let laid = kept::review(kept)?;
    if let Some(slot) = (laid.slot).filter(|slot| *slot != from && in_tree(editor, *slot)) {
        return Some(slot);
    }
    if let Some(ask) = ask_window(editor, kept).filter(|ask| *ask != from) {
        // Too small to split: the window below becomes the whole slot again.
        let below = split_below(editor, ask);
        if let Some(laid) = kept::review_mut(kept) {
            laid.slot = Some(ask);
            laid.ask = below;
        }
        return Some(ask);
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
/// `slot` and the window below it in `ask`, `asked` being the session one of them shows: the
/// frame is the layout again, with nothing to restore.
pub(super) fn carried(
    editor: &Editor,
    kept: &mut Vec<Kept>,
    review: ReviewId,
    window: WindowId,
    slot: WindowId,
    ask: Option<WindowId>,
    asked: Option<BufferId>,
) {
    if kept::review(kept).is_some() || !in_tree(editor, slot) || !in_tree(editor, window) {
        return;
    }
    let ask = ask.filter(|ask| *ask != slot && *ask != window && in_tree(editor, *ask));
    let pushed = kept::push_review(
        kept,
        ReviewKept {
            before: None,
            review,
            window,
            slot: Some(slot),
            from: None,
            ask,
            asked,
        },
    );
    debug_assert!(pushed, "one layout a tree");
}

/// The layout's slot, and the window below it `review-ask` showed a session in while it is
/// one, while the frame is the layout of `review`.
pub(super) fn slot_of(
    editor: &Editor,
    kept: &[Kept],
    review: ReviewId,
) -> Option<(WindowId, Option<WindowId>)> {
    let laid = kept::review(kept).filter(|laid| laid.review == review)?;
    let slot = kept::review_in_frame(kept).then_some(laid.slot).flatten()?;
    Some((slot, ask_window(editor, kept)))
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
    fit_slot_to(editor, slot, None);
}

/// Makes the window `slot` `permille` thousandths of the row it is in, else two fifths, by the
/// windows beside it, leaving whichever window is selected selected. A share is held to a
/// tenth from either edge, so that the review keeps room; a width the windows refuse stays.
pub(super) fn fit_slot_to(editor: &mut Editor, slot: WindowId, permille: Option<u16>) {
    let Some(((left, right), own)) = row_of(editor, slot) else {
        return;
    };
    let whole = right - left;
    let want = match permille {
        Some(permille) => (whole * usize::from(permille.clamp(100, 900)) + 500) / 1000,
        None => (whole * SLOT_FIFTHS + 2) / 5,
    };
    let (Ok(want), Ok(own)) = (isize::try_from(want), isize::try_from(own)) else {
        return;
    };
    let selected = editor.window().id();
    editor.select_window(slot);
    let _ = editor.resize_window(Way::Across, want - own);
    editor.select_window(selected);
}

/// The columns of the row the window `slot` is in, from the left edge of the windows beside it
/// to the right edge, and the columns it takes itself.
fn row_of(editor: &Editor, slot: WindowId) -> Option<((usize, usize), usize)> {
    let side = editor.side_window();
    let windows = editor.frame().windows();
    let regions = (windows.iter()).find_map(|(id, r)| (*id == slot).then_some(r))?;
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
    Some(((left, right), own))
}

/// The window `slot`'s share of the row it is in, in thousandths (ADR 0184).
pub(super) fn slot_share(editor: &Editor, slot: WindowId) -> Option<u16> {
    let ((left, right), own) = row_of(editor, slot)?;
    share(own, right - left)
}

/// The window `ask` below the slot `slot`: its share of the rows the two take, in thousandths.
pub(super) fn ask_share(editor: &Editor, slot: WindowId, ask: WindowId) -> Option<u16> {
    let (slot, ask) = (height(editor, slot)?, height(editor, ask)?);
    share(ask, slot + ask)
}

/// Makes the window `ask` below the slot `slot` `permille` thousandths of the rows the two
/// take, held to a tenth from either edge; a height the windows refuse stays.
pub(super) fn fit_ask_to(editor: &mut Editor, slot: WindowId, ask: WindowId, permille: u16) {
    let (Some(above), Some(own)) = (height(editor, slot), height(editor, ask)) else {
        return;
    };
    let want = ((above + own) * usize::from(permille.clamp(100, 900)) + 500) / 1000;
    let (Ok(want), Ok(own)) = (isize::try_from(want), isize::try_from(own)) else {
        return;
    };
    let selected = editor.window().id();
    editor.select_window(ask);
    let _ = editor.resize_window(Way::Down, want - own);
    editor.select_window(selected);
}

/// The rows the window `id` takes, its modeline's too.
fn height(editor: &Editor, id: WindowId) -> Option<usize> {
    (editor.frame().windows().iter()).find_map(|(window, regions)| {
        (*window == id).then(|| {
            let (top, bottom) = rows(regions);
            bottom - top
        })
    })
}

/// `part` of `whole` in thousandths, none of nothing.
fn share(part: usize, whole: usize) -> Option<u16> {
    let permille = (part * 1000 + whole / 2).checked_div(whole)?;
    u16::try_from(permille.min(1000)).ok()
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
