//! A project's files for `project-find-file` and its lines for `project-search` (ADR 0114): the
//! walk or the search a prompt asked for, started on its thread, and the batches it sends,
//! handed to the editor.
//!
//! Must not depend on: key presses or how a frame is drawn. The project is walked and searched
//! on those threads, never here, and nothing here logs what they found.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::{App, Loop, UserEvent, Waker};
use crate::grep::{Found, Grep};
use crate::walk::{Walked, Walker};

/// How long a walk may send nothing before the rows it sent that wait are added anyway, as a
/// list past 5000 adds them only once they are an eighth of it (ADR 0114).
const QUIET: Duration = Duration::from_millis(250);

/// The walk that sent a batch last, and when the rows waiting are added if it sends no more.
#[derive(Default)]
pub(super) struct Quiet(Option<(u64, Instant)>);

/// The walker for the app, waking the event loop with [`UserEvent::Walked`].
pub(super) fn walker(waker: &Waker) -> Walker {
    let waker = Mutex::new(waker.clone());
    Walker::new(super::home(), move |walked| {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::Walked(walked))
    })
}

/// The searcher for the app, waking the event loop with [`UserEvent::Grepped`].
pub(super) fn grep(waker: &Waker) -> Grep {
    let waker = Mutex::new(waker.clone());
    Grep::new(move |found| {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::Grepped(found))
    })
}

impl App {
    /// Starts the search a project's search prompt asked for, if any. The one before it, if one
    /// runs, has been stopped.
    pub(super) fn start_project_search(&mut self) {
        if let Some(request) = self.editor.take_search_request() {
            self.grep.search(request);
        }
    }

    /// A search sent rows: the prompt takes them in, if the search is still its newest.
    pub(super) fn grepped(&mut self, found: Found) {
        self.editor
            .rows_found(found.generation, found.rows, found.end);
        self.request_redraw();
    }

    /// Starts the walk a prompt that lists a project's files asked for, if any. The walk before
    /// it, if one runs, has been stopped.
    pub(super) fn start_walks(&mut self) {
        if let Some(request) = self.editor.take_walk_request() {
            self.walker.walk(request);
        }
    }

    /// The walker sent a batch: the prompt takes it in, if the walk is still its listing's, and
    /// a job may be left to add it past 5000 rows.
    pub(super) fn walked(&mut self, event_loop: &Loop<'_>, walked: Walked) {
        self.quiet_walk.0 = walked
            .end
            .is_none()
            .then(|| (walked.generation, Instant::now() + QUIET));
        self.editor
            .files_walked(walked.generation, walked.paths, walked.end);
        self.start_completions(event_loop);
        self.request_redraw();
    }

    /// A walk has sent nothing for [`QUIET`]: the rows it sent are added now, as a slow file
    /// system can hold the walk in one directory for long.
    pub(super) fn add_quiet_walk(&mut self, event_loop: &Loop<'_>) {
        let Some((generation, at)) = self.quiet_walk.0 else {
            return;
        };
        if Instant::now() < at {
            return;
        }
        self.quiet_walk.0 = None;
        self.editor.files_walked(generation, Vec::new(), None);
        self.start_completions(event_loop);
        self.request_redraw();
    }

    /// When the rows of a quiet walk are to be added, for the event loop's wake.
    pub(super) fn quiet_walk_deadline(&self) -> Option<Instant> {
        self.quiet_walk.0.map(|(_, at)| at)
    }
}
