//! A project's files for `project-find-file` (ADR 0114): the walk its prompt asked for, started
//! on the walker's thread, and the batches the walk sends, handed to the editor.
//!
//! Must not depend on: key presses or how a frame is drawn. The project is walked on the
//! walker's thread, never here, and nothing here logs what it found.

use std::sync::{Mutex, PoisonError};

use super::{App, Loop, UserEvent, Waker};
use crate::walk::{Walked, Walker};

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

impl App {
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
        self.editor
            .files_walked(walked.generation, walked.paths, walked.end);
        self.start_completions(event_loop);
        self.request_redraw();
    }
}
