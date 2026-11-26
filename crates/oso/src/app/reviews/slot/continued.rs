//! Continuing the review left last (ADR 0184): `review-layout` with no layout in front enters
//! that review's space, then shows it as it was left, from memory, or from its saved file once
//! oso knows its project and trusts it.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git: a saved
//! review opens as `review-resume` opens it, after its trust check.

use oso_core::editor::{Editor, ReviewId, WindowTree};

use super::super::super::App;
use super::super::keep::Continue;
use super::laid::PutBack;
use super::{IN_PROMPT, back, show_laid};
use crate::review::Content;
use crate::review::saved::{SavedLayout, SlotShows};
use crate::sessions::spaces::kept::{self, Kept};

impl App {
    /// Continues the review the person left last (ADR 0184): its space first, if it is still
    /// open, then the review as it was left, from memory, or from its saved file once oso knows
    /// its project and trusts it. Opens the review menu where no review was left or saved.
    pub(super) fn continue_review(&mut self) {
        self.note_front_left();
        let Some(found) = self.review_to_continue() else {
            if !self.reviews.keep.listed_once {
                log::info!("the saved reviews are not listed yet: the review menu opens");
            }
            return self.open_review_menu();
        };
        let gone = match found.space.as_deref() {
            Some(space) => !self.enter_space_of_review(space),
            None => false,
        };
        match found.what {
            Continue::Open(id) => self.continue_open_review(id),
            Continue::Saved { root, scope, pull } => {
                let commit = matches!(scope, crate::review::Scope::Commit { .. });
                if let Some(id) = self.resume_saved(&root, scope)
                    && let Some(index) = self.reviews.index(id)
                {
                    // A commit gone from a pull request's: that pull request's review instead.
                    let saving = &mut self.reviews.open[index].saving;
                    saving.instead = pull.filter(|_| commit);
                    // The layout its file kept goes back once a text shows.
                    saving.lay_kept = true;
                }
            }
        }
        if gone {
            self.editor
                .message("The review's space is no longer open: it continues in this one");
        }
        self.request_redraw();
    }

    /// A text of the review at `index` went in: the layout kept in its saved file goes back on
    /// the layout made for it, if it was continued from that file, once a side it showed can
    /// be asked for.
    pub(in crate::app::reviews) fn put_back_saved_layout(&mut self, index: usize) {
        let running = &self.reviews.open[index];
        if !running.saving.lay_kept || running.saving.waiting {
            return;
        }
        // Its layout left the frame before it could go back, as the person moved away: the
        // wait ends, and the layout stays kept as its file kept it.
        let kept = self.watch.spaces().kept();
        if kept::review(kept).is_none_or(|laid| laid.review != running.id)
            || !kept::review_in_frame(kept)
        {
            let saving = &mut self.reviews.open[index].saving;
            saving.lay_kept = false;
            saving.lay_carried = false;
            return;
        }
        if let Some(SavedLayout {
            slot: SlotShows::Side { path, .. },
            ..
        }) = &running.saving.layout
        {
            let reading = (running.decisions.reading(path)).map(|reading| match &reading.content {
                Content::Text { shape, .. } => Some(shape),
                _ => None,
            });
            let shown = (self.editor.review(running.id)).and_then(|model| model.file(path));
            // Waits, asking nothing, until the review shows the file's latest text, at each
            // text of the review; it ends as above once the layout left the frame.
            if !side_ready(reading, shown, running.read_once) {
                return;
            }
        }
        let saving = &mut self.reviews.open[index].saving;
        saving.lay_kept = false;
        let from = if std::mem::take(&mut saving.lay_carried) {
            PutBack::Carried
        } else {
            PutBack::File
        };
        self.put_back_layout(index, from);
    }

    /// Enters the open space whose id is `space`, unless it is in front; false when no open
    /// space has it.
    fn enter_space_of_review(&mut self, space: &str) -> bool {
        let spaces = self.watch.spaces();
        let at = (crate::sessions::spaces::SpaceId::parse(space))
            .and_then(|id| spaces.find(&id))
            .filter(|at| {
                spaces
                    .get(*at)
                    .is_some_and(crate::sessions::spaces::Space::is_open)
            });
        let Some(at) = at else {
            return false;
        };
        if spaces.current() != Some(at) {
            self.enter_space(at);
        }
        true
    }

    /// Lays out the open review `id` again where it was left: only selected again while its
    /// layout is in front, else in the layout in front, else in a layout made for it with its
    /// cursor and rows above put back.
    fn continue_open_review(&mut self, id: ReviewId) {
        let kept = self.watch.spaces_mut().kept_mut();
        let (shown, old) = continue_open(&mut self.editor, kept, id);
        self.freer.free(old);
        match shown {
            Shown::Selected => {}
            Shown::Laid => {
                if let Some(index) = self.reviews.index(id) {
                    let window = self.editor.tiled().0.id();
                    self.put_back_seen(index, window);
                    // The layout's own state as it was left (ADR 0184).
                    self.put_back_layout(index, super::laid::PutBack::Memory);
                }
            }
            Shown::Not => self.editor.message(IN_PROMPT),
        }
    }
}

/// How a review continued from memory was shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shown {
    /// Its layout was in front: the review is selected again, where it was.
    Selected,
    /// It is shown in the layout's window, or a layout made for it.
    Laid,
    /// Nothing changed: beneath a prompt.
    Not,
}

/// Shows the open review `id` as it continues (ADR 0184): selected again where it was left
/// while its layout is in the frame, out of a zoom made inside it first; else as an open
/// shows it. Gives the trees taken out of the frame, for the caller to free.
pub(super) fn continue_open(
    editor: &mut Editor,
    kept: &mut Vec<Kept>,
    id: ReviewId,
) -> (Shown, Vec<WindowTree>) {
    if editor.prompt().is_some() {
        return (Shown::Not, Vec::new());
    }
    if kept::review(kept).is_some_and(|laid| laid.review == id) {
        let mut old = Vec::new();
        if !kept::review_in_frame(kept) {
            match kept::zoom(editor, kept) {
                Ok((_, zoomed)) => old.extend(zoomed),
                Err(_) => return (Shown::Not, old),
            }
        }
        return match back(editor, kept) {
            Ok(again) => {
                old.extend(again);
                (Shown::Selected, old)
            }
            Err(_) => (Shown::Not, old),
        };
    }
    let (shown, old) = show_laid(editor, kept, id);
    let shown = if shown { Shown::Laid } else { Shown::Not };
    (shown, old.into_iter().collect())
}

/// Whether a side the slot showed may be asked for as a saved layout goes back (ADR 0184):
/// `reading` is the file's latest reading, `Some(None)` for one not of text, and `shown` the
/// text the review shows of it. Once the review shows its latest text, so that the jump is not
/// refused as read again; a reading not of text at once, the jump saying why; with no reading,
/// once every file was read, the file then gone from the review. A file read again keeps it
/// waiting, and nothing is asked meanwhile.
pub(in crate::app::reviews) fn side_ready<T>(
    reading: Option<Option<&std::sync::Arc<T>>>,
    shown: Option<&std::sync::Arc<T>>,
    read_once: bool,
) -> bool {
    match reading {
        None => read_once,
        Some(None) => true,
        Some(Some(text)) => shown.is_some_and(|shown| std::sync::Arc::ptr_eq(shown, text)),
    }
}
