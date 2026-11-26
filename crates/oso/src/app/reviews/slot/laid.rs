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
//! - The session below the slot is kept by its record name: put back while it is live and
//!   hosted, else named as ended; nothing here starts one.
//! - From a saved file, a slot's file outside the review's root is not opened; on a layout a
//!   restart carried, only what its slot lacks goes back.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{Editor, ReviewId};
use oso_core::review::Side;
use oso_core::selection::SelectionSet;
use oso_core::sum_tree::Bias;
use oso_core::window::{Scroll, WindowId};

use super::super::super::App;
use super::loaded::{SLOT_WITHHELD, SlotLoading, Source, source_fits};
use super::windows::{
    ask_share, ask_window, fit_ask_to, fit_slot_to, layout_window, slot, slot_share,
};
use crate::review::saved::{SavedLayout, SlotShows};
use crate::sessions::spaces::kept::{self, Kept};

/// What the slot's buffer is besides a file: a side of the review's, the message waiting, and
/// the files held as secret while a review was open; and the record name of a hosted session
/// whose buffer a buffer is.
pub(in crate::app::reviews) struct SlotKinds<'a> {
    /// The path and side of a buffer of the review's sides.
    pub(in crate::app::reviews) side: &'a dyn Fn(BufferId) -> Option<(Box<[u8]>, Side)>,
    pub(in crate::app::reviews) message: Option<BufferId>,
    pub(in crate::app::reviews) held: &'a HashSet<PathBuf>,
    pub(in crate::app::reviews) session: &'a dyn Fn(BufferId) -> Option<String>,
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
        session: super::below::session_shown(editor, kept, slot, kinds),
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
    seen: Option<&SlotSeen>,
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
    show_kept(editor, slot, layout, seen, show);
    editor.select_window(if layout.slot_selected { slot } else { window });
    true
}

/// Puts what the slot showed back alone on the layout of the review `id` a restart carried,
/// while it is the frame and its slot lacks it: its widths, selection and session are the
/// ones carried (ADR 0184). Returns whether it gave the slot anything.
pub(in crate::app::reviews) fn put_back_carried_slot(
    editor: &mut Editor,
    kept: &mut [Kept],
    (id, kinds): (ReviewId, &SlotKinds<'_>),
    layout: &SavedLayout,
    show: impl FnOnce(&mut Editor, &SlotShows) -> Option<BufferId>,
) -> bool {
    if kept::review(kept).is_none_or(|laid| laid.review != id) || !kept::review_in_frame(kept) {
        return false;
    }
    let Some(slot) = kept::review(kept)
        .and_then(|laid| laid.slot)
        .filter(|slot| super::windows::in_tree(editor, *slot))
    else {
        return false;
    };
    if !slot_lacks(&slot_shows(editor, slot, kinds), &layout.slot) {
        return false;
    }
    show_kept(editor, slot, layout, None, show);
    true
}

/// Whether a slot that shows `now` lacks what it showed, `then`: another file, side or kind.
/// A slot that showed nothing lacks nothing.
fn slot_lacks(now: &SlotShows, then: &SlotShows) -> bool {
    match (now, then) {
        (_, SlotShows::Nothing) | (SlotShows::Message, SlotShows::Message) => false,
        (SlotShows::Source { path, .. }, SlotShows::Source { path: was, .. }) => path != was,
        (
            SlotShows::Side { path, side, .. },
            SlotShows::Side {
                path: was,
                side: was_side,
                ..
            },
        ) => path != was || side != was_side,
        _ => true,
    }
}

/// Shows in `slot` what `show` gives for what the slot showed: with the cursors and view
/// `seen` kept in memory while that buffer did not change since, else at its line.
fn show_kept(
    editor: &mut Editor,
    slot: WindowId,
    layout: &SavedLayout,
    seen: Option<&SlotSeen>,
    show: impl FnOnce(&mut Editor, &SlotShows) -> Option<BufferId>,
) {
    let Some(buffer) = show(editor, &layout.slot) else {
        return;
    };
    editor.show_in(slot, buffer);
    let revision = editor.buffer(buffer).map(Buffer::revision);
    if let Some(seen) = seen.filter(|seen| seen.buffer == buffer && Some(seen.revision) == revision)
    {
        editor.restore_positions_in(slot, seen.selections.clone(), seen.scroll);
    } else if let SlotShows::Source {
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

/// The slot's own cursors, mark and goal and its view as the person left them, kept in memory
/// alone, never saved: put back as they were while the slot's buffer did not change since
/// (ADR 0184).
#[derive(Clone, Debug)]
pub(in crate::app::reviews) struct SlotSeen {
    buffer: BufferId,
    revision: u64,
    selections: SelectionSet,
    scroll: Scroll,
}

/// The slot's own cursors and view, while the layout of the review `id` is the frame and its
/// slot shows a buffer that may hold no secrets.
pub(in crate::app::reviews) fn slot_seen(
    editor: &Editor,
    kept: &[Kept],
    id: ReviewId,
) -> Option<SlotSeen> {
    let laid = kept::review(kept).filter(|laid| laid.review == id)?;
    let slot = (laid.slot)
        .filter(|slot| kept::review_in_frame(kept) && super::windows::in_tree(editor, *slot))?;
    let window = editor.window_by_id(slot)?;
    let buffer = editor.buffer(window.buffer())?;
    if buffer.is_secret() || buffer.withheld_from_agents() {
        return None;
    }
    Some(SlotSeen {
        buffer: window.buffer(),
        revision: buffer.revision(),
        selections: window.selections.clone(),
        scroll: window.scroll(),
    })
}

/// Whether `path`, a file the slot showed as a saved review kept it, lies under the review's
/// `root`, as the review's own files do: judged by its words, below the root, with no `..`.
pub(in crate::app::reviews) fn under_root(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|rest| {
        rest.components().next().is_some()
            && (rest.components()).all(|part| matches!(part, Component::Normal(_)))
    })
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

/// What the slot says of a file a saved review kept it showing outside the review's root,
/// which it does not show (ADR 0184).
const SLOT_OUTSIDE: &str =
    "The file the slot showed is outside the review's root, so it is not shown again";

/// Where a layout put back comes from (ADR 0184).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::reviews) enum PutBack {
    /// Kept in memory as the person left it in this run.
    Memory,
    /// Read from the review's saved file, as it was continued from it.
    File,
    /// Read from the saved file, onto the layout a restart carried: only what its slot lacks.
    Carried,
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
        let table = self.hosting.table();
        let session = |buffer| table.by_buffer(buffer).map(|hosted| hosted.name.clone());
        let kinds = SlotKinds {
            side: &side,
            message: self.waiting_message(),
            held,
            session: &session,
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
        let seen = slot_seen(&self.editor, self.watch.spaces().kept(), id);
        let saving = &mut self.reviews.open[index].saving;
        saving.slot_seen = seen;
        if saving.layout.as_ref() != Some(&state) {
            saving.layout = Some(state);
            saving.laid_changed = true;
        }
        saving.layout_read = false;
    }

    /// Puts the layout kept with the review at `index` back on the layout of it in front
    /// (ADR 0184): its slot's width, the window below it, what the slot showed and the side
    /// selected, then the session below it. A file not open is opened unshown, as a plain
    /// visit, and goes in once it loaded; nothing that may hold secrets is shown, and from a
    /// saved file, no file outside the review's root. On a layout a restart carried, only what
    /// its slot lacks goes back.
    pub(in crate::app::reviews) fn put_back_layout(&mut self, index: usize, from: PutBack) {
        let running = &self.reviews.open[index];
        let id = running.id;
        let Some(mut layout) = running.saving.layout.clone() else {
            return;
        };
        if from == PutBack::Carried {
            let Some(laid) = kept::review(self.watch.spaces().kept()).filter(|l| l.review == id)
            else {
                return;
            };
            // The selection carried stands, and nothing is asked for a slot that has it.
            layout.slot_selected = laid.slot == Some(self.editor.window().id());
            let lacks = (self.review_layout_state(running))
                .is_none_or(|now| slot_lacks(&now.slot, &layout.slot));
            if !lacks {
                return;
            }
        }
        let (ready, then) = match &layout.slot {
            SlotShows::Nothing => (None, Then::Nothing),
            SlotShows::Message => match self.waiting_message() {
                Some(buffer) => (Some(buffer), Then::Nothing),
                None => (None, Then::Message),
            },
            SlotShows::Source { path, .. }
                if (from != PutBack::Memory || running.saving.layout_read)
                    && !under_root(running.root.path(), path) =>
            {
                self.editor.message(SLOT_OUTSIDE);
                (None, Then::Nothing)
            }
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
        // The slot's own cursors and view, as the person left them in this run.
        let seen = (from == PutBack::Memory)
            .then(|| self.reviews.open[index].saving.slot_seen.clone())
            .flatten();
        let kept = self.watch.spaces_mut().kept_mut();
        let laid = if from == PutBack::Carried {
            let none = HashSet::new();
            let kinds = SlotKinds {
                side: &|_| None,
                message: None,
                held: &none,
                session: &|_| None,
            };
            // Judged above with the review's sides and message: here the slot is given it.
            put_back_carried_slot(&mut self.editor, kept, (id, &kinds), &layout, |_, _| ready)
        } else {
            lay_out_kept(
                &mut self.editor,
                kept,
                id,
                &layout,
                seen.as_ref(),
                |_, _| ready,
            )
        };
        if !laid {
            if let Then::Loading(buffer) = then {
                self.editor.kill_buffer(buffer);
            }
            return;
        }
        self.after_put_back(id, then, &layout);
        // A restart carried its session with its windows.
        if from != PutBack::Carried
            && let Some(name) = &layout.session
        {
            self.put_back_session(name, layout.ask_permille);
        }
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
}
