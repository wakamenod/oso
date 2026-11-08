//! The prompt that reads a path, served from the lister's threads, and the files commands asked
//! to visit (ADR 0087).
//!
//! Must not depend on: key presses or how a frame is drawn. Directories are listed on the
//! lister's threads, never here.

use std::sync::{Arc, Mutex, PoisonError};

use oso_core::complete::{Name, NameKind};

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
                directory,
                entries,
                truncated,
                ..
            } = answer
            else {
                continue;
            };
            let names = entries.ok().map(|entries| {
                // A name that is not UTF-8 is left out, as a path made from a lossy copy of it
                // would name another file.
                entries
                    .iter()
                    .filter_map(|entry| Some((entry, entry.name.to_str()?)))
                    .map(|(entry, name)| Name {
                        name: name.to_owned(),
                        kind: match entry.kind {
                            Kind::File => NameKind::File,
                            Kind::Directory => NameKind::Directory,
                            Kind::Link => NameKind::Link,
                            Kind::Other => NameKind::Other,
                        },
                    })
                    .collect::<Arc<[Name]>>()
            });
            self.editor.file_listed(directory, names, truncated);
            changed = true;
        }
        self.serve_files();
        if changed {
            self.request_redraw();
        }
    }

    /// The prompt that read a path has closed: what waits for its listings goes.
    pub(super) fn forget_listings(&mut self) {
        // Only once it has closed: a question over it must not drop what it waits for.
        if !self.editor.file_prompt_open() {
            self.lister.forget();
        }
    }
}
