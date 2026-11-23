//! Where a review's message and questions go (ADR 0122, step 12a; ADR 0163): the session the
//! person picked for the review, the space's orchestrator while the review is of one of its
//! repositories, else the session in the review's worktree; and the review's worktree and
//! scope as the texts sent name them.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here sends: `send` and
//! `ask` do, through the sessions' delivery path.

use std::path::{Path, PathBuf};

use oso_core::editor::ReviewId;

use super::super::App;
use super::super::sessions::Orchestrator;
use super::scope_label;
use crate::commands::review::{MAX_NAME, one_line};
use crate::review::pr::PrFound;
use crate::sessions::worker::SessionKey;

impl App {
    /// The orchestrator `review` talks to: the space in front's, while the review's root, or
    /// the worktree its pull request's head is checked out in, is one of that space's.
    pub(super) fn review_orchestrator(&self, review: ReviewId) -> Option<Orchestrator> {
        let index = self.reviews.index(review)?;
        let running = &self.reviews.open[index];
        let root = self.editor.project(running.project)?.root();
        let places: Vec<&Path> = places(running.pr.as_ref(), root).collect();
        self.space_orchestrator_of(&places)
    }

    /// The session the person picked for `review` with `C-u C-c C-c`, while it runs.
    pub(super) fn picked_session(&self, review: ReviewId) -> Option<SessionKey> {
        (self.reviews.sending.picked.get(&review).copied())
            .filter(|key| self.hosting.table().get(*key).is_some_and(|s| s.runs()))
    }

    /// The session `review` talks to outside a space with an orchestrator (step 12a): the one
    /// the person picked for it while it runs, else the one in each of [`places`], the one
    /// shown last first.
    pub(super) fn review_session(&self, review: ReviewId) -> Option<SessionKey> {
        let picked = self.picked_session(review);
        picked.or_else(|| {
            let index = self.reviews.index(review)?;
            let running = &self.reviews.open[index];
            let root = self.editor.project(running.project)?.root();
            places(running.pr.as_ref(), root).find_map(|place| self.worktree_session(place))
        })
    }

    /// Session `key`'s name, as the echo area shows it.
    pub(super) fn review_session_name(&self, key: SessionKey) -> String {
        (self.hosting.table().get(key)).map_or_else(String::new, |s| one_line(&s.name, 40))
    }

    /// The worktree the texts of the review at `index` name: its root, or for a pull request
    /// the worktree its head is checked out in.
    pub(super) fn review_worktree(&self, index: usize) -> Option<PathBuf> {
        let running = &self.reviews.open[index];
        let root = self.editor.project(running.project)?.root();
        places(running.pr.as_ref(), root)
            .next()
            .map(Path::to_path_buf)
    }

    /// The scope of the review at `index` as the texts sent name it: `uncommitted, in oso`.
    pub(super) fn review_scope(&self, index: usize) -> Option<String> {
        let running = &self.reviews.open[index];
        let found = self.editor.project(running.project)?;
        Some(format!(
            "{}, in {}",
            scope_label(&running.scope),
            one_line(found.name(), MAX_NAME)
        ))
    }

    /// Why `orchestrator`, which a text of a review goes to, takes nothing now, with the key
    /// that resumes or starts it, and what then becomes of the text, `left`.
    pub(super) fn not_running(&self, orchestrator: &Orchestrator, left: &str) -> Option<String> {
        let (name, how) = match orchestrator {
            Orchestrator::Running(_) => return None,
            Orchestrator::Stopped { name, key } => (
                name,
                format!(
                    "{} in its {} resumes it",
                    self.dispatcher.where_is(&self.editor, "session-resume"),
                    // A native one's buffer is its transcript (ADR 0176, part O-2).
                    match self.hosting.natives.has(*key) {
                        true => "transcript",
                        false => "terminal",
                    }
                ),
            ),
            Orchestrator::NoTerminal { name } => (
                name,
                format!(
                    "{} starts it",
                    self.dispatcher
                        .where_is(&self.editor, "space-orchestrator-start")
                ),
            ),
        };
        Some(format!("Orchestrator {name} is not running: {how}; {left}"))
    }
}

/// Where a review's session is looked for, in order: a pull request's comments go to the
/// session where its head branch is checked out (step 12b, Q11), else to the review's own.
pub(super) fn places<'a>(
    found: Option<&'a PrFound>,
    root: &'a Path,
) -> impl Iterator<Item = &'a Path> {
    let head = found.and_then(|found| found.worktree.as_deref());
    head.into_iter().chain(std::iter::once(root))
}
