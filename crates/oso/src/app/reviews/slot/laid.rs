//! The review layout's own state (ADR 0184): the slot's width, the window below it, which side
//! was selected and what the slot showed, read as the review is left laid out and put back on
//! the layout made for it as it continues.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here opens a file or runs
//! git: the host finds or opens what the slot shows, and this shows the buffer it gives.
//!
//! - What the slot showed is never a buffer that may hold secrets or is withheld from agents,
//!   nor a file held as secret while a review was open: it is kept as nothing.
//! - A line past the end goes to the last; a share is held to a tenth from either edge.

use std::collections::HashSet;
use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{Editor, ReviewId};
use oso_core::review::Side;
use oso_core::selection::SelectionSet;
use oso_core::sum_tree::Bias;
use oso_core::window::{Scroll, WindowId};

use super::super::super::App;
use super::windows::{
    ask_share, ask_window, fit_ask_to, fit_slot_to, layout_window, slot, slot_share,
};
use crate::review::Content;
use crate::review::saved::{SavedLayout, SlotShows};
use crate::sessions::spaces::kept::{self, Kept};

/// What the slot's buffer is besides a file: a side of the review's, the message waiting, and
/// the files held as secret while a review was open.
pub(in crate::app::reviews) struct SlotKinds<'a> {
    /// The path and side of a buffer of the review's sides.
    pub(in crate::app::reviews) side: &'a dyn Fn(BufferId) -> Option<(Box<[u8]>, Side)>,
    pub(in crate::app::reviews) message: Option<BufferId>,
    pub(in crate::app::reviews) held: &'a HashSet<PathBuf>,
}

/// The state of the layout of the review `id`, while it is the frame: None otherwise.
pub(in crate::app::reviews) fn layout_state(
    editor: &Editor,
    kept: &[Kept],
    id: ReviewId,
    kinds: &SlotKinds<'_>,
) -> Option<SavedLayout> {
    let laid = kept::review(kept).filter(|laid| laid.review == id)?;
    if !kept::review_in_frame(kept) {
        return None;
    }
    let slot = laid
        .slot
        .filter(|slot| super::windows::in_tree(editor, *slot))?;
    let ask = ask_window(editor, kept);
    Some(SavedLayout {
        slot_selected: editor.window().id() == slot,
        slot_permille: slot_share(editor, slot)?,
        ask_permille: ask.and_then(|ask| ask_share(editor, slot, ask)),
        slot: slot_shows(editor, slot, kinds),
    })
}

/// What the window `slot` shows, as a layout keeps it.
pub(in crate::app::reviews) fn slot_shows(
    editor: &Editor,
    slot: WindowId,
    kinds: &SlotKinds<'_>,
) -> SlotShows {
    let Some(window) = editor.window_by_id(slot) else {
        return SlotShows::Nothing;
    };
    let id = window.buffer();
    let Some(buffer) = editor.buffer(id) else {
        return SlotShows::Nothing;
    };
    if buffer.is_secret() || buffer.withheld_from_agents() {
        return SlotShows::Nothing;
    }
    if kinds.message == Some(id) {
        return SlotShows::Message;
    }
    let text = buffer.text();
    let cursor = text.offset_to_point(window.selections.primary().cursor);
    let top = text.offset_to_point(window.scroll().start).row;
    let (Ok(line), Ok(above)) = (
        u32::try_from(cursor.row + 1),
        u32::try_from(cursor.row.saturating_sub(top)),
    ) else {
        return SlotShows::Nothing;
    };
    if let Some((path, side)) = (kinds.side)(id) {
        return SlotShows::Side {
            path,
            side,
            line,
            above,
        };
    }
    let path = buffer.path().filter(|_| !buffer.is_internal());
    match path {
        Some(path)
            if path.to_str().is_some()
                && !Buffer::name_may_hold_secrets(path)
                && !kinds.held.contains(path) =>
        {
            SlotShows::Source {
                path: path.to_path_buf(),
                line,
                column: u32::try_from(cursor.column).unwrap_or(0),
                above,
            }
        }
        _ => SlotShows::Nothing,
    }
}

/// Puts `layout` on the layout of the review `id` while it is the frame: the slot, made again
/// if it is gone, gets its width, the window below it its height, and what `show` gives for
/// what the slot showed, at its line; then the side that was selected is. Returns whether the
/// layout of `id` was the frame.
pub(in crate::app::reviews) fn lay_out_kept(
    editor: &mut Editor,
    kept: &mut [Kept],
    id: ReviewId,
    layout: &SavedLayout,
    show: impl FnOnce(&mut Editor, &SlotShows) -> Option<BufferId>,
) -> bool {
    if kept::review(kept).is_none_or(|laid| laid.review != id) || !kept::review_in_frame(kept) {
        return false;
    }
    let Some(window) = layout_window(editor, kept) else {
        return false;
    };
    let Some(slot) = slot(editor, kept, window) else {
        return false;
    };
    fit_slot_to(editor, slot, Some(layout.slot_permille));
    if let (Some(permille), Some(ask)) = (layout.ask_permille, ask_window(editor, kept)) {
        fit_ask_to(editor, slot, ask, permille);
    }
    if let Some(buffer) = show(editor, &layout.slot) {
        editor.show_in(slot, buffer);
        if let SlotShows::Source {
            line,
            column,
            above,
            ..
        } = layout.slot
        {
            put_at(editor, slot, (line, column), above);
        } else if let SlotShows::Side { line, above, .. } = layout.slot {
            put_at(editor, slot, (line, 0), above);
        }
    }
    editor.select_window(if layout.slot_selected { slot } else { window });
    true
}

/// Shows `buffer` in the slot of the layout of the review `id` while it is the frame and the
/// slot still shows `filler`, the buffer it was given while `buffer` loaded, at `line` from 1,
/// `column` bytes in and `above` rows below the top. Returns whether it did.
pub(in crate::app::reviews) fn show_loaded(
    editor: &mut Editor,
    kept: &[Kept],
    (id, filler): (ReviewId, BufferId),
    buffer: BufferId,
    (line, column): (u32, u32),
    above: u32,
) -> bool {
    let Some(laid) = kept::review(kept).filter(|laid| laid.review == id) else {
        return false;
    };
    let slot = laid
        .slot
        .filter(|slot| super::windows::in_tree(editor, *slot));
    let Some(slot) = slot.filter(|_| kept::review_in_frame(kept)) else {
        return false;
    };
    if editor
        .window_by_id(slot)
        .map(oso_core::window::Window::buffer)
        != Some(filler)
    {
        return false;
    }
    editor.show_in(slot, buffer);
    put_at(editor, slot, (line, column), above);
    true
}

/// Whether a file the slot showed may be shown in it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::reviews) enum Source {
    /// Its name, or its open buffer, may hold secrets, or it was held as secret in this run.
    Withheld,
    /// Its open buffer.
    Open(BufferId),
    /// Not open: a plain visit opens it.
    Closed,
}

/// Whether the file at `path` may be shown in the slot again, judged again as it is put back
/// (ADR 0184): by its name now, the files `held` as secret in this run, and its open buffer.
pub(in crate::app::reviews) fn source_fits(
    editor: &Editor,
    held: &HashSet<PathBuf>,
    path: &std::path::Path,
) -> Source {
    if Buffer::name_may_hold_secrets(path) || held.contains(path) {
        return Source::Withheld;
    }
    match crate::commands::review::open_buffer_of(editor, path) {
        Some(open) => {
            let fits = (editor.buffer(open))
                .is_some_and(|buffer| !buffer.is_secret() && !buffer.withheld_from_agents());
            if fits {
                Source::Open(open)
            } else {
                Source::Withheld
            }
        }
        None => Source::Closed,
    }
}

/// Puts `window`'s cursor alone at `column` bytes into line `line` from 1, each as far as the
/// text goes, with `above` rows above it. O(log n).
pub(in crate::app::reviews) fn put_at(
    editor: &mut Editor,
    window: WindowId,
    (line, column): (u32, u32),
    above: u32,
) {
    let Some(shown) =
        (editor.window_by_id(window)).and_then(|window| editor.buffer(window.buffer()))
    else {
        return;
    };
    let text = shown.text();
    let row = (line.max(1) as usize - 1).min(text.max_point().row);
    let start = text.line_start(row);
    let offset = text.clip_offset(start + text.line_len(row).min(column as usize), Bias::Left);
    let scroll = Scroll {
        start: text.line_start(row.saturating_sub(above as usize)),
        left: 0,
    };
    editor.restore_positions_in(window, SelectionSet::new(offset), scroll);
}

/// What the slot says of a file it showed that may now hold secrets, which it does not show.
const SLOT_WITHHELD: &str = "The file the slot showed may hold secrets, so it is not shown again";

/// A file the slot showed, loading to be shown again in it once its text came (ADR 0184).
#[derive(Debug)]
pub(in crate::app::reviews) struct SlotLoading {
    review: ReviewId,
    buffer: BufferId,
    /// What the slot shows meanwhile, which it must still show for the file to go in.
    filler: BufferId,
    line: u32,
    column: u32,
    above: u32,
}

/// What the slot is given as a layout is put back, besides a buffer at once.
enum Then {
    Nothing,
    /// A file loading.
    Loading(BufferId),
    /// The message made again from the comments not sent.
    Message,
    /// A side asked of the review's thread.
    Side,
}

impl App {
    /// The buffer of the message waiting to be sent, while it is open.
    pub(in crate::app::reviews) fn waiting_message(&self) -> Option<BufferId> {
        (self.reviews.sending.message.as_ref())
            .map(|message| message.buffer)
            .filter(|buffer| self.editor.buffer(*buffer).is_some())
    }

    /// The state of the layout of `running`'s review, while it is the frame (ADR 0184).
    pub(in crate::app::reviews) fn review_layout_state(
        &self,
        running: &super::super::state::Running,
    ) -> Option<SavedLayout> {
        let root = running.root.path();
        let held = &self.reviews.keep.held;
        let side = |buffer| {
            super::super::visit::side_of(&running.sides, buffer)
                .filter(|(path, _)| !super::super::secrets::withheld(root, held, path))
        };
        let kinds = SlotKinds {
            side: &side,
            message: self.waiting_message(),
            held,
        };
        layout_state(&self.editor, self.watch.spaces().kept(), running.id, &kinds)
    }

    /// Keeps the state of the review layout in front with its review, before it goes out of
    /// the frame: `q`, `review-layout` and a space switch (ADR 0184).
    pub(in crate::app) fn keep_review_layout_state(&mut self) {
        let Some(id) = kept::review(self.watch.spaces().kept()).map(|laid| laid.review) else {
            return;
        };
        // The layout its saved file kept, not put back yet, stands.
        let Some(index) = self
            .reviews
            .index(id)
            .filter(|at| !self.reviews.open[*at].saving.lay_kept)
        else {
            return;
        };
        let Some(state) = self.review_layout_state(&self.reviews.open[index]) else {
            return;
        };
        let saving = &mut self.reviews.open[index].saving;
        if saving.layout.as_ref() != Some(&state) {
            saving.layout = Some(state);
            saving.laid_changed = true;
        }
    }

    /// A text of the review at `index` went in: the layout kept in its saved file goes back on
    /// the layout made for it, if it was continued from that file, once a side it showed can
    /// be asked for.
    pub(in crate::app::reviews) fn put_back_saved_layout(&mut self, index: usize) {
        let running = &self.reviews.open[index];
        if !running.saving.lay_kept || running.saving.waiting {
            return;
        }
        if let Some(SavedLayout {
            slot: SlotShows::Side { path, .. },
            ..
        }) = &running.saving.layout
        {
            let shown = (running.decisions.reading(path)).is_some_and(|reading| {
                let Content::Text { shape, .. } = &reading.content else {
                    return true;
                };
                (self.editor.review(running.id))
                    .and_then(|model| model.file(path))
                    .is_some_and(|file| std::sync::Arc::ptr_eq(file, shape))
            });
            if !shown && !running.read_once {
                return;
            }
        }
        self.reviews.open[index].saving.lay_kept = false;
        self.put_back_layout(index);
    }

    /// Puts the layout kept with the review at `index` back on the layout of it in front
    /// (ADR 0184): its slot's width, the window below it, what the slot showed and the side
    /// selected. A file not open is opened unshown, as a plain visit, and goes in once it
    /// loaded; nothing that may hold secrets is shown.
    pub(in crate::app::reviews) fn put_back_layout(&mut self, index: usize) {
        let running = &self.reviews.open[index];
        let id = running.id;
        let Some(layout) = running.saving.layout.clone() else {
            return;
        };
        let (ready, then) = match &layout.slot {
            SlotShows::Nothing => (None, Then::Nothing),
            SlotShows::Message => match self.waiting_message() {
                Some(buffer) => (Some(buffer), Then::Nothing),
                None => (None, Then::Message),
            },
            SlotShows::Source { path, .. } => self.slot_source(path),
            SlotShows::Side { path, side, .. } => {
                let root = running.root.path();
                if super::super::secrets::withheld(root, &self.reviews.keep.held, path) {
                    (None, Then::Nothing)
                } else {
                    match super::super::visit::side_shown(&running.sides, path, *side) {
                        Some(buffer) => (Some(buffer), Then::Nothing),
                        None => (None, Then::Side),
                    }
                }
            }
        };
        let kept = self.watch.spaces_mut().kept_mut();
        if !lay_out_kept(&mut self.editor, kept, id, &layout, |_, _| ready) {
            if let Then::Loading(buffer) = then {
                self.editor.kill_buffer(buffer);
            }
            return;
        }
        self.after_put_back(id, then, &layout);
        self.request_redraw();
    }

    /// What follows a layout put back of the review `id` for what its slot is given `then`:
    /// a file loading waits to go in, the message is made again, a side is asked for.
    fn after_put_back(&mut self, id: ReviewId, then: Then, layout: &SavedLayout) {
        match (then, &layout.slot) {
            (
                Then::Loading(buffer),
                SlotShows::Source {
                    line,
                    column,
                    above,
                    ..
                },
            ) => {
                let slot = kept::review(self.watch.spaces().kept()).and_then(|laid| laid.slot);
                let filler = slot.and_then(|slot| self.editor.window_by_id(slot));
                if let Some(filler) = filler.map(oso_core::window::Window::buffer) {
                    self.reviews.keep.slot_loading = Some(SlotLoading {
                        review: id,
                        buffer,
                        filler,
                        line: *line,
                        column: *column,
                        above: *above,
                    });
                }
            }
            (Then::Message, _) => {
                // Made again from the comments not sent: what was typed in it is not kept.
                let selected = self.editor.window().id();
                self.send_review(id);
                if !layout.slot_selected && self.editor.window_by_id(selected).is_some() {
                    self.editor.select_window(selected);
                }
            }
            (
                Then::Side,
                SlotShows::Side {
                    path,
                    side,
                    line,
                    above,
                },
            ) => {
                // Asked from the review's window, which the answer must find selected.
                let selected = self.editor.window().id();
                let kept = self.watch.spaces_mut().kept_mut();
                if let Some(window) = layout_window(&self.editor, kept) {
                    self.editor.select_window(window);
                }
                let path = path.clone();
                self.visit_review_line(id, &path, (*side, *line), oso_core::editor::VisitTo::Side);
                match self
                    .reviews
                    .index(id)
                    .and_then(|at| self.reviews.open[at].jumping.as_mut())
                {
                    Some(jumping) => jumping.put_back(*above, layout.slot_selected),
                    None => {
                        if self.editor.window_by_id(selected).is_some() {
                            self.editor.select_window(selected);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The buffer of the file at `path` a slot put back shows: the open one, or one opened
    /// unshown as a plain visit, with one name (ADR 0085), to go in once it loaded. Nothing
    /// for a name or buffer that may hold secrets, or a file held as secret in this run.
    fn slot_source(&mut self, path: &std::path::Path) -> (Option<BufferId>, Then) {
        match source_fits(&self.editor, &self.reviews.keep.held, path) {
            Source::Withheld => {
                self.editor.message(SLOT_WITHHELD);
                (None, Then::Nothing)
            }
            Source::Open(open) => (Some(open), Then::Nothing),
            Source::Closed => {
                match self.visit_with(path.to_path_buf(), oso_core::load::UNASKED, true) {
                    Some(buffer) => (None, Then::Loading(buffer)),
                    None => (None, Then::Nothing),
                }
            }
        }
    }

    /// The load of `buffer` ended with its text: a file a layout put back goes in its slot, if
    /// the slot still shows what it was given meanwhile, and nothing of one that opened as
    /// holding secrets shows. One not shown goes.
    pub(in crate::app) fn review_slot_loaded(&mut self, buffer: BufferId) {
        let Some(loading) = self
            .reviews
            .keep
            .slot_loading
            .take_if(|at| at.buffer == buffer)
        else {
            return;
        };
        let fits = (self.editor.buffer(buffer))
            .is_some_and(|shown| !shown.is_secret() && !shown.withheld_from_agents());
        let shown = fits
            && show_loaded(
                &mut self.editor,
                self.watch.spaces().kept(),
                (loading.review, loading.filler),
                buffer,
                (loading.line, loading.column),
                loading.above,
            );
        if !fits {
            self.editor.message(SLOT_WITHHELD);
        }
        let modified = (self.editor.buffer(buffer)).is_some_and(Buffer::is_modified);
        if !shown && !modified && !(self.editor.windows()).any(|window| window.buffer() == buffer) {
            self.editor.kill_buffer(buffer);
        }
        self.request_redraw();
    }

    /// Whether the load at `index`, of `buffer`, found no file where a file a layout put back
    /// was: the buffer goes, unshown, the slot keeps what it shows, and the echo area says so
    /// (ADR 0184), rather than a new file opening.
    pub(in crate::app) fn review_slot_gone(
        &mut self,
        index: usize,
        buffer: BufferId,
        update: &crate::loader::LoadUpdate,
    ) -> bool {
        use crate::loader::{LoadError, LoadUpdate};
        let slot = &mut self.reviews.keep.slot_loading;
        if !matches!(update, LoadUpdate::Failed(LoadError::NotFound))
            || slot.take_if(|at| at.buffer == buffer).is_none()
        {
            return false;
        }
        let name = (self.editor.buffer(buffer))
            .and_then(Buffer::path)
            .map(|path| crate::commands::review::one_line(&path.to_string_lossy(), 200));
        self.loads.swap_remove(index);
        self.editor.kill_buffer(buffer);
        self.editor.message(format!(
            "The file the slot showed no longer exists: {}",
            name.unwrap_or_default()
        ));
        true
    }
}
