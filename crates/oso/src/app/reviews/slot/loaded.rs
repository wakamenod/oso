//! A file the review layout's slot showed, put back (ADR 0184): judged again by its name, the
//! files held as secret in this run and its open buffer; one not open loads unshown and goes in
//! once its text came, if the slot still shows what it was given meanwhile; one gone says so.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here opens a file: the
//! host's plain visit does, and opening never executes anything.

use std::collections::HashSet;
use std::path::PathBuf;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{Editor, ReviewId};

use super::super::super::App;
use super::laid::put_at;
use crate::sessions::spaces::kept::{self, Kept};

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

/// What the slot says of a file it showed that may now hold secrets, which it does not show.
pub(super) const SLOT_WITHHELD: &str =
    "The file the slot showed may hold secrets, so it is not shown again";

/// A file the slot showed, loading to be shown again in it once its text came (ADR 0184).
#[derive(Debug)]
pub(in crate::app::reviews) struct SlotLoading {
    pub(super) review: ReviewId,
    pub(super) buffer: BufferId,
    /// What the slot shows meanwhile, which it must still show for the file to go in.
    pub(super) filler: BufferId,
    pub(super) line: u32,
    pub(super) column: u32,
    pub(super) above: u32,
}

impl App {
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
