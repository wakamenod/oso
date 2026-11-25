//! Opening a review in the app (ADR 0122): a review of a scope made, or the one open shown and
//! read again, and where it shows: in the review layout, the selected window, or nowhere yet
//! (ADR 0167).
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git: the review's
//! own thread does, started here.

use oso_core::editor::ReviewId;
use oso_core::project::ProjectId;

use super::ReviewEvent;
use super::scope::{same_scope, scope_label};
use super::state::{Reopen, Running};
use crate::app::{App, UserEvent};
use crate::commands::review::{MAX_NAME, RESTRICTED, one_line};
use crate::git::TrustedRoot;
use crate::review::view::{Tell, View};
use crate::review::{Request, Scope};

/// Where a review opened shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Show {
    /// In the review layout, as a person's open (ADR 0160, Q2).
    Laid,
    /// In the selected window, as a restart opens it again.
    Window,
    /// Nowhere yet, as a step opens a commit's review before showing it (ADR 0167).
    Hidden,
}

impl App {
    /// Opens a review of `scope` in `project`, or shows the one open and reads it again: as a
    /// request names it, or as a saved review or a restart kept it, a session review with its
    /// own baseline (step 6); and gives its id. `Show::Laid`, as a person's open, shows it in
    /// the review layout (ADR 0160, Q2); `Show::Window`, as a restart opens it again, in the
    /// selected window; `Show::Hidden`, as a step through a pull request's commits opens one
    /// before showing it (ADR 0167), nowhere.
    pub(super) fn open_scope(
        &mut self,
        project: ProjectId,
        scope: Scope,
        show: Show,
    ) -> Option<ReviewId> {
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            self.editor.message(RESTRICTED);
            return None;
        };
        if matches!(scope, Scope::Session { .. }) {
            self.session_review_opened(root.path());
        }
        let window = self.editor.tiled().0.id();
        let open =
            self.reviews.open.iter().position(|running| {
                running.project == project && same_scope(&running.scope, &scope)
            });
        if let Some(index) = open {
            let id = self.reviews.open[index].id;
            if show != Show::Hidden
                && !(show == Show::Laid && self.show_review_laid(id))
                && let Some(buffer) = self.editor.review_buffer(id)
            {
                self.editor.show_review_unified(window);
                self.editor.show_in(window, buffer);
            }
            let running = &mut self.reviews.open[index];
            let reopen = running.reopen(&scope);
            // A new baseline: the session review compares with it from now on.
            running.scope = scope.clone();
            match reopen {
                // A revoked review starts over with the scope, as `g` would.
                Reopen::Refresh | Reopen::Restart => self.refresh_review(id, true),
                Reopen::Open => self.ask_review(index, Request::Open(scope)),
            }
            self.request_redraw();
            return Some(id);
        }
        let name = self
            .editor
            .project(project)
            .map_or_else(String::new, |project| one_line(project.name(), MAX_NAME));
        let label = scope_label(&scope);
        let (id, buffer) = self
            .editor
            .add_review(project, format!("*review: {label} {name}*"));
        let waker = self.waker.clone();
        let view = View::start(
            format!("Review \u{b7} {label} \u{b7} {name}"),
            move |text| {
                waker.send(UserEvent::Review(ReviewEvent::Text {
                    review: id,
                    text: Box::new(text),
                }))
            },
        );
        let view = match view {
            Ok(view) => view,
            Err(e) => {
                self.editor.remove_review(id);
                self.editor.message(format!("Cannot start the review: {e}"));
                return None;
            }
        };
        // The top its files' names are judged under, before highlighting is ever on.
        let top = root.path().to_path_buf();
        view.tell(Tell::Highlighter {
            highlighter: None,
            top,
        });
        let running = Running::new(id, project, scope.clone(), root, view);
        self.reviews.open.push(running);
        if show != Show::Hidden && !(show == Show::Laid && self.show_review_laid(id)) {
            // In the whole area of a pair of a review's panes there, as in the layout.
            self.editor.show_review_unified(window);
            self.editor.show_in(window, buffer);
        }
        let index = self.reviews.open.len() - 1;
        self.start_reviewer(index);
        // What was saved of it is read meanwhile, and what its thread reports waits for it
        // (step 6).
        self.open_saved(index, &scope);
        self.ask_review(index, Request::Open(scope));
        self.request_redraw();
        Some(id)
    }
}
