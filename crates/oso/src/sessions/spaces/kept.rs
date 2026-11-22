//! The trees kept behind the one in the frame (ADR 0121, ADR 0160): each space's, and the
//! frame's while no space is current, a stack whose last entry was made last. `space-zoom`
//! keeps the tree from before it, and the review layout keeps the tree from before it with
//! the windows it is made of, so that the two nest: a zoom inside the layout zooms back out to
//! the layout, and the layout's restore drops a zoom made inside it.
//!
//! Must not depend on: `app`, `dispatch` or `config`; this holds trees, window names and the
//! name of the buffer `review-ask` showed only.

use oso_core::buffer::BufferId;
use oso_core::editor::{Editor, ReviewId, WindowTree};
use oso_core::window::WindowId;

use super::{Refused, Zoom};

/// A tree kept behind the frame's, and why.
#[derive(Debug)]
pub enum Kept {
    /// The tree from before `space-zoom`.
    Zoom(WindowTree),
    /// The review layout's.
    Review(ReviewKept),
}

/// What the review layout keeps (ADR 0160): the tree it restores and the windows it is made
/// of, by name, never by place, so that a window moved by hand is still found, and the session
/// `review-ask` showed last (ADR 0169).
#[derive(Debug)]
pub struct ReviewKept {
    /// The tree from before the layout, or none when a restart carried the layout: a restart
    /// hands over only the tree in the frame (ADR 0121).
    pub before: Option<WindowTree>,
    /// The review the layout shows last.
    pub review: ReviewId,
    /// The window at its left that shows the review, a pair's first while it shows two panes.
    pub window: WindowId,
    /// The window at its right that a jump shows its source in, while it is in the tree.
    pub slot: Option<WindowId>,
    /// The review's window a jump was asked in last, which the way back selects.
    pub from: Option<WindowId>,
    /// The window below the slot that `review-ask` shows a session in while the slot shows
    /// something else, while it is in the tree (ADR 0169).
    pub ask: Option<WindowId>,
    /// The session's buffer `review-ask` showed last, its terminal or native buffer, so that a
    /// jump while it fills the slot keeps it below.
    pub asked: Option<BufferId>,
}

/// The review layout's entry of `kept`, if it holds one.
pub fn review(kept: &[Kept]) -> Option<&ReviewKept> {
    kept.iter().rev().find_map(|kept| match kept {
        Kept::Review(review) => Some(review),
        Kept::Zoom(_) => None,
    })
}

/// The review layout's entry of `kept`, if it holds one, to change.
pub fn review_mut(kept: &mut [Kept]) -> Option<&mut ReviewKept> {
    kept.iter_mut().rev().find_map(|kept| match kept {
        Kept::Review(review) => Some(review),
        Kept::Zoom(_) => None,
    })
}

/// Whether the frame's tree is the review layout's itself: its entry is the last, with no
/// zoom made inside it.
pub fn review_in_frame(kept: &[Kept]) -> bool {
    matches!(kept.last(), Some(Kept::Review(_)))
}

/// Puts `review` on `kept`, unless it holds a layout's entry already: one layout a tree.
/// Returns whether it did.
pub fn push_review(kept: &mut Vec<Kept>, review: ReviewKept) -> bool {
    if self::review(kept).is_some() {
        return false;
    }
    kept.push(Kept::Review(review));
    true
}

/// Takes the review layout's entry out of `kept`, with the zooms made inside it, given back
/// for the caller to free off the UI thread: a zoom's tree may hold large wraps.
pub fn take_review(kept: &mut Vec<Kept>) -> Option<(ReviewKept, Vec<Kept>)> {
    let at = kept
        .iter()
        .rposition(|kept| matches!(kept, Kept::Review(_)))?;
    let mut taken = kept.split_off(at).into_iter();
    match taken.next() {
        Some(Kept::Review(review)) => Some((review, taken.collect())),
        _ => None,
    }
}

/// `space-zoom` over the trees `kept` behind the frame's: the tree's selected window alone,
/// and back, with that window's buffer and places put in the tree's window it was zoomed from.
/// With the side window selected, the tree's window is selected first.
///
/// # Errors
///
/// [`Refused`] while a prompt is open.
///
/// Zooming out gives the zoomed tree it took out, for the caller to free off the UI thread.
pub fn zoom(
    editor: &mut Editor,
    kept: &mut Vec<Kept>,
) -> Result<(Zoom, Option<WindowTree>), Refused> {
    if editor.prompt().is_some() {
        return Err(Refused);
    }
    let side = editor.side_window();
    if side == Some(editor.window().id()) {
        editor.other_window(1);
    }
    let Some(Kept::Zoom(saved)) = kept.pop_if(|kept| matches!(kept, Kept::Zoom(_))) else {
        let lone = editor.lone_window_tree();
        let saved = editor.swap_window_tree(lone).map_err(|_| Refused)?;
        kept.push(Kept::Zoom(saved));
        return Ok((Zoom::In, None));
    };
    let window = editor.window();
    let (buffer, selections, scroll) =
        (window.buffer(), window.selections.clone(), window.scroll());
    let zoomed = match editor.swap_window_tree(saved) {
        Ok(zoomed) => zoomed,
        Err(saved) => {
            kept.push(Kept::Zoom(saved));
            return Err(Refused);
        }
    };
    let back = editor.window().id();
    editor.show_in(back, buffer);
    editor.restore_positions_in(back, selections, scroll);
    Ok((Zoom::Out, Some(zoomed)))
}

#[cfg(test)]
#[path = "kept_tests.rs"]
mod tests;
