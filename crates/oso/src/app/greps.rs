//! A project's lines for `project-search` (ADR 0114): the search its prompt asked for, started on
//! the searching thread, and the rows the search sends, handed to the editor.
//!
//! Must not depend on: key presses or how a frame is drawn. The project is searched on the
//! searching threads, never here, and nothing here logs what they found.

use std::sync::{Mutex, PoisonError};

use super::{App, Loop, UserEvent, Waker};
use crate::grep::{Found, Grep};

/// The searcher for the app, waking the event loop with [`UserEvent::Grepped`].
pub(super) fn grep(waker: &Waker) -> Grep {
    let waker = Mutex::new(waker.clone());
    Grep::new(super::home(), move |found| {
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

    /// A search sent rows: the prompt takes them in, if the search is still its newest, and
    /// many rows of an older one are left to the worker to free.
    pub(super) fn grepped(&mut self, event_loop: &Loop<'_>, found: Found) {
        self.editor
            .rows_found(found.generation, found.rows, found.end);
        self.start_completions(event_loop);
        self.request_redraw();
    }
}
