//! The review layout (ADR 0160): the review at the left of the frame and one window at its
//! right, the slot, which shows one thing at a time: the source a jump goes to, a side shown
//! read-only, the message of comments to send, and later a space orchestrator's answer. One
//! key, `review-layout`, shows it, goes back from the slot to the review where it was, and
//! restores the windows from before it, as `q` in the review does.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git: showing a
//! review is what any window showing it does.
//!
//! - The tree from before is taken out whole, as `space-zoom` takes it ([`Kept`]), and comes
//!   back exactly, its windows' cursors and views kept. It is kept with the space's trees, so
//!   each space has its own layout, and a zoom inside the layout nests.
//! - The layout's windows are known by their names, never by their places, so that a window
//!   the person moved, split or resized is still found. A slot deleted by hand is made again
//!   by the next jump, at the review's right, two fifths of the row wide.
//! - A restart hands over only the tree in the frame (ADR 0121): the layout comes back as its
//!   windows, still the layout, with nothing to restore; `q` then leaves the review's window
//!   alone with what it showed behind the review.
//!
//! The choices are free functions over the editor and the trees kept behind the frame's, so
//! that they are tested without an app; [`App`]'s methods say what they did.

use oso_core::buffer::BufferId;
use oso_core::editor::{Editor, ReviewId, WindowTree};
use oso_core::window::WindowId;

use super::super::App;
use crate::sessions::spaces::kept::{self, Kept, ReviewKept};

mod windows;

use windows::{
    carried, fit_slot, in_slot, in_tree, jump_slot, layout_window, show_in_slot, shows_review,
    slot_of,
};

/// The slot's share of the row it is in, in fifths: the review keeps three (ADR 0160, Q4).
const SLOT_FIFTHS: usize = 2;

/// What the layout's key says while a prompt is open.
const IN_PROMPT: &str = "Not in the minibuffer";

/// What `review-layout` does where the person is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
    /// Open the review menu.
    Menu,
    /// The layout was made for this review.
    Entered,
    /// The windows from before it came back.
    Restored,
    /// The review is selected again, from the slot.
    Back,
    /// Nothing changed, for this reason.
    Said(&'static str),
}

impl App {
    /// `review-layout`: with `menu`, or with no review open, the review menu; else the layout,
    /// the way back to the review from the slot, or the restore from the review.
    pub(super) fn review_layout_key(&mut self, menu: bool) {
        let kept = self.watch.spaces_mut().kept_mut();
        let (key, old) = press(&mut self.editor, kept, menu);
        self.freer.free(old);
        match key {
            Key::Menu => self.open_review_menu(),
            Key::Said(why) => self.editor.message(why),
            Key::Entered | Key::Restored | Key::Back => self.request_redraw(),
        }
    }

    /// `q` buried a review: the windows from before the layout come back, if the frame is one.
    pub(super) fn review_quit_layout(&mut self) {
        if kept::review(self.watch.spaces().kept()).is_some() {
            self.restore_review_layout();
        }
    }

    /// The review `id` was closed: closing the layout's review restores the windows from
    /// before it, as `q` does (ADR 0160, Q8).
    pub(super) fn review_closed_layout(&mut self, id: ReviewId) {
        let laid = kept::review(self.watch.spaces().kept()).is_some_and(|kept| kept.review == id);
        if laid {
            self.restore_review_layout();
        }
    }

    fn restore_review_layout(&mut self) {
        let kept = self.watch.spaces_mut().kept_mut();
        match restore(&mut self.editor, kept) {
            Ok(old) => {
                self.freer.free(old);
                self.request_redraw();
            }
            Err(why) => self.editor.message(why),
        }
    }

    /// Opens the review menu, as `C-x v r` did before the layout, and starts its counts; in a
    /// space whose orchestrator is recorded, the start that asks for its repository first
    /// (ADR 0163).
    fn open_review_menu(&mut self) {
        if !self.start_review_in_space() {
            self.open_review_menu_now();
        }
    }

    /// Opens the review menu, and starts its counts.
    pub(super) fn open_review_menu_now(&mut self) {
        let effects = self.dispatcher.open_menu_named(&mut self.editor, "review");
        self.sync_review_menu();
        if effects.redraw {
            self.request_redraw();
        }
    }

    /// Shows the review `id` as a person's open does (ADR 0160, Q2): in the layout's window
    /// while the frame is the layout, else in a layout made for it. Returns false, showing
    /// nothing, where it does not, as beneath a prompt or zoomed inside the layout.
    pub(super) fn show_review_laid(&mut self, id: ReviewId) -> bool {
        let kept = self.watch.spaces_mut().kept_mut();
        let (shown, old) = show_laid(&mut self.editor, kept, id);
        self.freer.free(old);
        shown
    }

    /// Selects the layout's slot for a jump's source while the frame is the layout. Returns
    /// whether it did.
    pub(super) fn select_jump_slot(&mut self) -> bool {
        let kept = self.watch.spaces_mut().kept_mut();
        jump_slot(&mut self.editor, kept)
    }

    /// Shows `buffer` in the slot while the frame is the review layout, selecting it with
    /// `select`: the message of comments, and later an orchestrator's answer. Returns whether
    /// it did.
    pub(in crate::app) fn show_in_review_slot(&mut self, buffer: BufferId, select: bool) -> bool {
        let kept = self.watch.spaces_mut().kept_mut();
        let shown = show_in_slot(&mut self.editor, kept, buffer, select);
        if shown {
            self.request_redraw();
        }
        shown
    }

    /// Whether the selected window is the layout's slot, while the frame is the layout.
    pub(in crate::app) fn in_review_slot(&self) -> bool {
        in_slot(&self.editor, self.watch.spaces().kept())
    }

    /// Goes back from the slot to the review, as `review-layout` does there.
    pub(in crate::app) fn back_from_review_slot(&mut self) {
        let kept = self.watch.spaces_mut().kept_mut();
        if in_slot(&self.editor, kept) {
            let old = back(&mut self.editor, kept).unwrap_or_default();
            self.freer.free(old);
            self.request_redraw();
        }
    }

    /// A restart carried the review `review` shown in `window`, the layout's, with its slot in
    /// `slot`: the frame is the layout again, with nothing to restore.
    pub(super) fn carried_review_layout(
        &mut self,
        review: ReviewId,
        window: WindowId,
        slot: WindowId,
    ) {
        let kept = self.watch.spaces_mut().kept_mut();
        carried(&self.editor, kept, review, window, slot);
    }

    /// The layout's slot, for a restart to name, while the frame is the layout of `review`.
    pub(in crate::app) fn review_layout_slot(&self, review: ReviewId) -> Option<WindowId> {
        slot_of(self.watch.spaces().kept(), review)
    }
}

/// The review whose buffer `buffer` is: its own, a pane's, or its files pane's.
pub(super) fn owner(editor: &Editor, buffer: BufferId) -> Option<ReviewId> {
    crate::commands::review::review_shown(editor, buffer)
}

/// What `review-layout` does, and the trees it took out of the frame, for the caller to free
/// off the UI thread.
pub(super) fn press(
    editor: &mut Editor,
    kept: &mut Vec<Kept>,
    menu: bool,
) -> (Key, Vec<WindowTree>) {
    if editor.prompt().is_some() {
        return (Key::Said(IN_PROMPT), Vec::new());
    }
    if menu || editor.reviews().next().is_none() {
        return (Key::Menu, Vec::new());
    }
    if kept::review(kept).is_none() {
        let key = match to_lay_out(editor) {
            Some(review) if enter(editor, kept, review) => Key::Entered,
            Some(_) => Key::Said(IN_PROMPT),
            None => Key::Menu,
        };
        return (key, Vec::new());
    }
    // In the review, or the layout's review closed meanwhile, as beneath a prompt.
    let closed = kept::review(kept).is_some_and(|laid| editor.review_buffer(laid.review).is_none());
    if closed || owner(editor, editor.tiled().0.buffer()).is_some() {
        return match restore(editor, kept) {
            Ok(old) => (Key::Restored, old),
            Err(why) => (Key::Said(why), Vec::new()),
        };
    }
    // A zoom made inside the layout: out of it first, back to the layout's tree.
    let mut old = Vec::new();
    if !kept::review_in_frame(kept) {
        match kept::zoom(editor, kept) {
            Ok((_, zoomed)) => old.extend(zoomed),
            Err(_) => return (Key::Said(IN_PROMPT), old),
        }
    }
    match back(editor, kept) {
        Ok(again) => {
            old.extend(again);
            (Key::Back, old)
        }
        Err(why) => (Key::Said(why), old),
    }
}

/// The open review the layout shows: the one the selected window showed last of the selected
/// buffer's project, else of any, else the one of that project opened last, else the one
/// opened last.
fn to_lay_out(editor: &Editor) -> Option<ReviewId> {
    let project = crate::commands::review::selected_project(editor);
    let project_of = |review: ReviewId| {
        (editor.reviews()).find_map(|(id, project)| (id == review).then_some(project))
    };
    let (window, _) = editor.tiled();
    let shown = std::iter::once(window.buffer()).chain(window.places().map(|(id, _)| id));
    let mut any = None;
    for buffer in shown {
        let Some(review) = owner(editor, buffer) else {
            continue;
        };
        if project.is_some() && project_of(review) == project {
            return Some(review);
        }
        any = any.or(Some(review));
    }
    let last_of_project = (editor.reviews())
        .filter(|(_, of)| Some(*of) == project)
        .map(|(id, _)| id)
        .last();
    (last_of_project.or(any)).or_else(|| editor.reviews().map(|(id, _)| id).last())
}

/// Shows the review `id` in the layout's window while the frame is the layout, else in a
/// layout made for it; the tree a layout made again took out, for the caller to free.
pub(super) fn show_laid(
    editor: &mut Editor,
    kept: &mut Vec<Kept>,
    id: ReviewId,
) -> (bool, Option<WindowTree>) {
    let Some(buffer) = editor.review_buffer(id) else {
        return (false, None);
    };
    if kept::review(kept).is_none() {
        return (enter(editor, kept, id), None);
    }
    if !kept::review_in_frame(kept) {
        // Zoomed inside the layout: the zoomed window shows it.
        return (false, None);
    }
    if let Some(laid) = kept::review_mut(kept) {
        laid.review = id;
    }
    match layout_window(editor, kept) {
        Some(window) => {
            editor.select_window(window);
            editor.show_in(window, buffer);
            (true, None)
        }
        None => match lay_out_again(editor, kept) {
            Some(old) => (true, Some(old)),
            None => (false, None),
        },
    }
}

/// Takes the frame's tree out, keeping it, and shows the review `id` at the left and the slot
/// at the right, the review selected. Returns whether it did: not beneath a prompt.
pub(super) fn enter(editor: &mut Editor, kept: &mut Vec<Kept>, id: ReviewId) -> bool {
    let Some(buffer) = editor.review_buffer(id) else {
        return false;
    };
    let slot_buffer = first_slot_buffer(editor);
    let Some((window, slot, before)) = swap_in_layout(editor, buffer, slot_buffer) else {
        return false;
    };
    let pushed = kept::push_review(
        kept,
        ReviewKept {
            before: Some(before),
            review: id,
            window,
            slot: Some(slot),
            from: None,
        },
    );
    debug_assert!(pushed, "one layout a tree");
    true
}

/// Puts a tree of the review's `buffer` and `slot_buffer` side by side in the frame, the slot
/// two fifths wide, and gives the review's window, the slot and the tree taken out.
fn swap_in_layout(
    editor: &mut Editor,
    buffer: BufferId,
    slot_buffer: BufferId,
) -> Option<(WindowId, WindowId, WindowTree)> {
    if editor.prompt().is_some() || buffer == slot_buffer {
        return None;
    }
    // Both exist, so the tree is of two windows.
    if editor.buffer(buffer).is_none() || editor.buffer(slot_buffer).is_none() {
        return None;
    }
    let tree = editor.window_tree_showing(&[buffer, slot_buffer]);
    let before = editor.swap_window_tree(tree).ok()?;
    let window = editor.tiled().0.id();
    let side = editor.side_window();
    let slot = (editor.frame().windows().iter())
        .map(|(id, _)| *id)
        .find(|id| *id != window && Some(*id) != side);
    let Some(slot) = slot else {
        // Not reached, as both buffers exist; the frame's tree comes back. The tree dropped is
        // of fresh windows, whose wraps are provisional, so it is cheap to drop here.
        if let Ok(fresh) = editor.swap_window_tree(before) {
            drop(fresh);
        }
        return None;
    };
    fit_slot(editor, slot);
    editor.select_window(window);
    Some((window, slot, before))
}

/// The layout's review is shown by no window any more: a new layout tree in place of the
/// frame's, keeping the tree from before, its slot showing what the selected window does.
/// Gives the frame's tree it took out.
fn lay_out_again(editor: &mut Editor, kept: &mut [Kept]) -> Option<WindowTree> {
    let id = kept::review(kept)?.review;
    let buffer = editor.review_buffer(id)?;
    let here = editor.tiled().0.buffer();
    let slot_buffer = if owner(editor, here).is_some() {
        first_slot_buffer(editor)
    } else {
        here
    };
    let (window, slot, old) = swap_in_layout(editor, buffer, slot_buffer)?;
    if let Some(laid) = kept::review_mut(kept) {
        laid.window = window;
        laid.slot = Some(slot);
        laid.from = None;
    }
    Some(old)
}

/// What the slot shows first: what the selected window shows, else the buffer it showed last
/// that is no review's, else `*Messages*`, which every editor has. Never a buffer that may hold
/// secrets or is withheld from agents, which would come back on screen unasked (ADR 0044).
fn first_slot_buffer(editor: &Editor) -> BufferId {
    let (window, _) = editor.tiled();
    let fits = |id: BufferId| {
        editor.buffer(id).is_some_and(|buffer| {
            !buffer.is_secret() && !buffer.withheld_from_agents() && owner(editor, id).is_none()
        })
    };
    std::iter::once(window.buffer())
        .chain(window.places().map(|(id, _)| id))
        .find(|id| fits(*id))
        .unwrap_or_else(|| editor.messages_buffer())
}

/// Restores the windows from before the layout, dropping a zoom made inside it, and gives the
/// trees it took out of the frame, for the caller to free off the UI thread.
///
/// # Errors
///
/// What to say while a prompt is open: nothing changed.
pub(super) fn restore(
    editor: &mut Editor,
    kept: &mut Vec<Kept>,
) -> Result<Vec<WindowTree>, &'static str> {
    if editor.prompt().is_some() {
        return Err(IN_PROMPT);
    }
    let Some((laid, inside)) = kept::take_review(kept) else {
        return Ok(Vec::new());
    };
    // The zooms made inside the layout, the layout's own tree among them.
    let mut old: Vec<WindowTree> = (inside.into_iter())
        .filter_map(|inside| match inside {
            Kept::Zoom(tree) => Some(tree),
            Kept::Review(review) => review.before,
        })
        .collect();
    let Some(before) = laid.before else {
        // A restart carried the layout, and not the tree from before it.
        if in_tree(editor, laid.window) {
            editor.select_window(laid.window);
        }
        editor.delete_other_windows();
        let window = editor.tiled().0.id();
        if owner(editor, editor.tiled().0.buffer()).is_some() {
            editor.show_review_unified(window);
            let _ = editor.walk_buffers(1);
        }
        return Ok(old);
    };
    match editor.swap_window_tree(before) {
        Ok(layout) => {
            old.push(layout);
            Ok(old)
        }
        Err(before) => {
            // Refused only beneath a prompt, which was looked at: kept, to try again, the
            // zooms inside it given up.
            let pushed = kept::push_review(
                kept,
                ReviewKept {
                    before: Some(before),
                    ..laid
                },
            );
            debug_assert!(pushed, "one layout a tree");
            Err(IN_PROMPT)
        }
    }
}

/// Selects the layout's review where the person left it: the window a jump was asked in last,
/// else the layout's own, else any window showing the review, else a layout made again, whose
/// tree taken out it gives.
///
/// # Errors
///
/// What to say when the layout's review is closed.
pub(super) fn back(
    editor: &mut Editor,
    kept: &mut [Kept],
) -> Result<Option<WindowTree>, &'static str> {
    let Some(laid) = kept::review(kept) else {
        return Ok(None);
    };
    let found = [laid.from, Some(laid.window)]
        .into_iter()
        .flatten()
        .find(|id| shows_review(editor, *id));
    if let Some(id) = found {
        editor.select_window(id);
        return Ok(None);
    }
    if let Some(id) = layout_window(editor, kept) {
        editor.select_window(id);
        return Ok(None);
    }
    lay_out_again(editor, kept)
        .map(Some)
        .ok_or("The review is closed: C-x v r opens one")
}

#[cfg(test)]
#[path = "slot_tests.rs"]
mod tests;
