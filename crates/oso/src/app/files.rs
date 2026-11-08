//! The prompt that reads a path, served from the lister's threads, and the files commands asked
//! to visit (ADR 0087).
//!
//! Must not depend on: key presses or how a frame is drawn. Directories are listed on the
//! lister's threads, never here.

use std::sync::{Arc, Mutex, PoisonError};

use oso_core::complete::path::{Name, NameKind};

use super::{App, UserEvent, Waker};
use crate::lister::{Answer, Kind, Lister};

/// The lister for the app, so that its limits hold for the process, waking the event loop with
/// [`UserEvent::Listed`].
pub(super) fn lister(waker: &Waker) -> Lister {
    let waker = Mutex::new(waker.clone());
    Lister::new(move || {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::Listed)
    })
}

impl App {
    /// Starts what commands asked for files: the directories the prompt that reads a path needs
    /// listed, and the files to visit.
    pub(super) fn serve_files(&mut self) {
        for directory in self.editor.take_file_requests() {
            // A generation is not needed: listings are kept by directory for the prompt's life.
            self.lister.list(directory, 0);
        }
        for (path, limit) in self.editor.take_visits() {
            if let Some(buffer) = self.visit(path, limit) {
                self.editor.show(buffer);
            }
            self.request_redraw();
        }
    }

    /// The lister answered: gives the prompt what it found.
    pub(super) fn listed(&mut self) {
        let mut changed = false;
        for answer in self.lister.take() {
            let Answer::Listed {
                directory, entries, ..
            } = answer
            else {
                continue;
            };
            let names = entries.ok().map(|entries| {
                entries
                    .iter()
                    .map(|entry| Name {
                        name: entry.name.to_string_lossy().into_owned(),
                        kind: match entry.kind {
                            Kind::File => NameKind::File,
                            Kind::Directory => NameKind::Directory,
                            Kind::Link => NameKind::Link,
                            Kind::Other => NameKind::Other,
                        },
                    })
                    .collect::<Arc<[Name]>>()
            });
            self.editor.file_listed(directory, names);
            changed = true;
        }
        self.serve_files();
        if changed {
            self.request_redraw();
        }
    }

    /// The prompt that read a path has closed: what waits for its listings goes.
    pub(super) fn forget_listings(&mut self) {
        if !self.editor.files_pending() && self.editor.file_directory().is_none() {
            self.lister.forget();
        }
    }
}
