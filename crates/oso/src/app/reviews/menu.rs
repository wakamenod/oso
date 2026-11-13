//! The review menu's counts in the app (ADR 0117 "Live content", ADR 0122): started on the
//! counting threads as the menu opens for a trusted project, stopped as it closes, and each
//! count put in the editor for the menu's notes; the session's count once a baseline comes
//! while the menu is open; and the branch's base, found on the same threads when a review of
//! the branch is asked for before it was known.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git.

use std::sync::{Mutex, PoisonError};

use oso_core::editor::{Count, ScopeKind};

use oso_core::project::ProjectId;

use super::super::{App, UserEvent, Waker};
use super::{RESTRICTED, ReviewEvent, state};
use crate::commands::review::review_project;
use crate::review::count::{Counted, Counter, Counting, Job};
use crate::review::{Scope, TrustedRoot};

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
        let Some(project) = review_project(&self.editor) else {
            return;
        };
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return;
        };
        // Kept so that the counts stop running git if the project stops being trusted.
        self.reviews.counted = Some((project, root.clone()));
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
            if self.reviews.base_wanted == Some(project) {
                self.reviews.base_wanted = None;
                match &base {
                    Some(found) => {
                        self.open_review(project, ScopeKind::Branch, Some(found.clone()));
                    }
                    None => self.editor.message("No base was found: C-u b names it"),
                }
            }
            self.editor.set_branch_base(project, generation, base);
            shown = true;
        }
        if shown {
            self.request_redraw();
        }
    }

    /// Finds the branch's base on the counting threads, for a review of the branch asked for
    /// before it was known; the review opens once it is found.
    pub(super) fn find_branch_base(&mut self, project: ProjectId) {
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(RESTRICTED);
        };
        self.reviews.counted = Some((project, root.clone()));
        self.reviews.base_wanted = Some(project);
        let generation = self
            .editor
            .start_scope_counts(project, &[ScopeKind::Branch]);
        let job = Job {
            generation,
            project,
            root,
            counting: Counting::Branch,
        };
        let waker = &self.waker;
        self.reviews
            .counter
            .get_or_insert_with(|| counter(waker))
            .start(vec![job]);
        self.editor.message("Finding the branch's base\u{2026}");
    }

    /// Counts the session scope of `project`, whose baseline just came, while the menu is open
    /// for it.
    pub(super) fn count_session(&mut self, project: ProjectId) {
        let Some(baseline) = self.reviews.baseline(project) else {
            return;
        };
        let (Some((counted, root)), Some(menu)) =
            (&self.reviews.counted, self.editor.scope_menu(project))
        else {
            return;
        };
        if !self.reviews.menu_open || *counted != project {
            return;
        }
        let generation = menu.generation();
        let job = Job {
            generation,
            project,
            root: root.clone(),
            counting: Counting::Scope(ScopeKind::Session, Scope::Session { baseline }),
        };
        self.editor
            .set_scope_count(project, generation, ScopeKind::Session, Count::Running);
        let waker = &self.waker;
        self.reviews
            .counter
            .get_or_insert_with(|| counter(waker))
            .add(vec![job]);
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
