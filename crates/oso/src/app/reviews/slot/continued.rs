//! Continuing the review left last (ADR 0184): `review-layout` with no layout in front enters
//! that review's space, then shows it as it was left, from memory, or from its saved file once
//! oso knows its project and trusts it.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git: a saved
//! review opens as `review-resume` opens it, after its trust check.

use oso_core::editor::{Editor, ReviewId, WindowTree};

use super::super::super::App;
use super::super::keep::Continue;
use super::{IN_PROMPT, back, show_laid};
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
                    self.put_back_layout(index);
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
