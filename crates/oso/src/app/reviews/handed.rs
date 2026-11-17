//! The reviews a restart opens again (ADR 0122, step 6, Q-H): the instance going names each
//! review its windows show, by its project's root and scope, and the new one opens each again
//! in the first of those windows once it knows the project and trusts it, from what the old
//! instance saved before it went.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git: opening a
//! review does, on its own thread, only in a trusted project.
//!
//! - Until a review opens, its windows show what they showed behind it.
//! - The windows left over, a pair's second, are closed: the review's own layout makes them
//!   again, as a pair's two windows are always the same size, so no window is paired from
//!   outside.
//! - A review whose project is not known or trusted within [`WAIT`] is not opened.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use oso_core::window::WindowId;

use crate::app::App;
use crate::carry;
use crate::carry::reviews::ReviewEntry;
use crate::git::TrustedRoot;
use crate::review::Scope;

/// How long a review a restart carried waits for its project to be known and trusted.
const WAIT: Duration = Duration::from_secs(30);

/// A review a restart carried, waiting to open again.
#[derive(Debug)]
pub(super) struct Handed {
    root: PathBuf,
    scope: Scope,
    windows: Vec<WindowId>,
    until: Instant,
}

impl App {
    /// The reviews this instance's windows may show, for a restart to open again: each its
    /// root, scope and id, whose windows the state finds.
    pub(in crate::app) fn reviews_to_hand_over(&self) -> Vec<carry::reviews::Shown> {
        (self.reviews.open.iter())
            .filter(|running| !running.revoked)
            .map(|running| carry::reviews::Shown {
                root: running.root.path().to_owned(),
                scope: running.scope.clone(),
                review: running.id,
            })
            .collect()
    }

    /// A restart carried `entries`: each waits for its project, by the windows that showed it.
    pub(in crate::app) fn reviews_handed_over(&mut self, entries: Vec<ReviewEntry>) {
        let windows: Vec<WindowId> = (carry::tiled_windows(&self.editor).iter())
            .map(|window| window.id())
            .collect();
        let until = Instant::now() + WAIT;
        for entry in entries {
            let shown: Vec<WindowId> = (entry.windows.iter())
                .filter_map(|at| windows.get(*at).copied())
                .collect();
            if !shown.is_empty() {
                self.reviews.handed.push(Handed {
                    root: entry.root,
                    scope: entry.scope,
                    windows: shown,
                    until,
                });
            }
        }
    }

    /// After each event: opens each review a restart carried whose project is now known and
    /// trusted, and lets go of those that waited past [`WAIT`].
    pub(super) fn reopen_handed_reviews(&mut self) {
        if self.reviews.handed.is_empty() || self.editor.prompt().is_some() {
            return;
        }
        let now = Instant::now();
        let mut index = 0;
        while index < self.reviews.handed.len() {
            let handed = &self.reviews.handed[index];
            let project = (self.project_with_root(&handed.root))
                .filter(|project| TrustedRoot::of(&self.editor, *project).is_some());
            match project {
                Some(project) => {
                    let handed = self.reviews.handed.remove(index);
                    self.reopen_handed(project, handed);
                }
                None if handed.until <= now => {
                    log::info!(
                        "a review a restart carried did not open: its project is not trusted"
                    );
                    self.reviews.handed.remove(index);
                }
                None => index += 1,
            }
        }
    }

    /// Opens `handed` in `project` in the first window that showed it, and closes the others.
    fn reopen_handed(&mut self, project: oso_core::project::ProjectId, handed: Handed) {
        let Handed { scope, windows, .. } = handed;
        let selected = self.editor.tiled().0.id();
        let Some(&first) = (windows.iter()).find(|id| self.editor.window_by_id(**id).is_some())
        else {
            return;
        };
        self.editor.select_window(first);
        // A session review compares with the baseline it had (step 6).
        self.open_scope(project, scope);
        for window in windows.into_iter().filter(|id| *id != first) {
            if self.editor.window_by_id(window).is_some() {
                self.editor.select_window(window);
                if self.editor.tiled().0.id() == window {
                    let _ = self.editor.delete_window();
                }
            }
        }
        let back = if self.editor.window_by_id(selected).is_some() {
            selected
        } else {
            first
        };
        self.editor.select_window(back);
        self.request_redraw();
    }
}
