//! Each file's project (ADR 0114): asking the thread that finds them, and taking in its answers.
//!
//! Must not depend on: key presses or how a frame is drawn. Projects are found on their own
//! thread, never here.

use std::sync::{Mutex, PoisonError};

use super::{App, UserEvent, Waker};
use crate::project::Detector;

/// The thread that finds projects, waking the event loop with [`UserEvent::Projects`]. It never
/// takes the home directory, as the environment names it, for a project.
pub(super) fn detector(waker: &Waker) -> Detector {
    let waker = Mutex::new(waker.clone());
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    Detector::new(home, move || {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::Projects)
    })
}

impl App {
    /// Hands the files whose project is wanted to the thread.
    pub(super) fn find_projects(&mut self) {
        for request in self.editor.take_project_requests() {
            self.projects.find(request);
        }
    }

    /// The thread found projects: gives them to their buffers, and asks for a frame, as the
    /// modeline may name one.
    pub(super) fn projects_found(&mut self) {
        let answers = self.projects.take();
        if answers.is_empty() {
            return;
        }
        for (request, found) in answers {
            self.editor.project_found(&request, found);
        }
        self.request_redraw();
    }
}
