//! The review menu's counts in the app (ADR 0117 "Live content", ADR 0122): started on the
//! counting threads as the menu opens for a trusted project, stopped as it closes, and each
//! count put in the editor for the menu's notes.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git.

use std::sync::{Mutex, PoisonError};

use oso_core::editor::{Editor, ScopeKind};
use oso_core::project::ProjectId;

use super::super::{App, UserEvent, Waker};
use super::{ReviewEvent, state};
use crate::review::count::{Counted, Counter, Counting, Job};
use crate::review::{Scope, TrustedRoot};

/// The project the menu opens for: the review's when a review buffer is current, else the
/// current buffer's.
pub(super) fn menu_project(editor: &Editor) -> Option<ProjectId> {
    let buffer = editor.tiled().0.buffer();
    if let Some(review) = editor.review_of(buffer) {
        return editor
            .reviews()
            .find_map(|(id, project)| (id == review).then_some(project));
    }
    editor.buffer(buffer)?.project()
}

impl App {
    /// Starts the menu's counts as the review menu opens, and stops them as it closes.
    pub(super) fn sync_review_menu(&mut self) {
        let open = self.dispatcher.menu_name() == Some("review");
        if open == self.reviews.menu_open {
            return;
        }
        self.reviews.menu_open = open;
        if !open {
            if let Some(counter) = &self.reviews.counter {
                counter.stop();
            }
            return;
        }
        let Some(project) = menu_project(&self.editor) else {
            return;
        };
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return;
        };
        let baseline = self.reviews.baseline(project);
        let mut scopes = Vec::with_capacity(5);
        let mut countings = Vec::with_capacity(5);
        if let Some(baseline) = baseline {
            scopes.push(ScopeKind::Session);
            countings.push(Counting::Scope(
                ScopeKind::Session,
                Scope::Session { baseline },
            ));
        }
        for (kind, scope) in [
            (ScopeKind::Uncommitted, Scope::Uncommitted),
            (ScopeKind::Unstaged, Scope::Unstaged),
            (ScopeKind::Staged, Scope::Staged),
        ] {
            scopes.push(kind);
            countings.push(Counting::Scope(kind, scope));
        }
        scopes.push(ScopeKind::Branch);
        countings.push(Counting::Branch);
        let generation = self.editor.start_scope_counts(project, &scopes);
        let jobs = countings
            .into_iter()
            .map(|counting| Job {
                generation,
                project,
                root: root.clone(),
                counting,
            })
            .collect();
        let waker = &self.waker;
        self.reviews
            .counter
            .get_or_insert_with(|| counter(waker))
            .start(jobs);
        self.request_redraw();
    }

    pub(super) fn scope_counted(&mut self, counted: Counted) {
        let Counted {
            generation,
            project,
            scope,
            count,
            base,
        } = counted;
        let mut shown = self
            .editor
            .set_scope_count(project, generation, scope, count);
        if let Some(base) = base {
            self.editor.set_branch_base(project, generation, base);
            shown = true;
        }
        if shown {
            self.request_redraw();
        }
    }
}

/// The menu's counting threads, waking the event loop with each count.
fn counter(waker: &Waker) -> Counter {
    let waker = Mutex::new(waker.clone());
    Counter::new(state(), move |counted| {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::Review(ReviewEvent::Counted(counted)));
    })
}
