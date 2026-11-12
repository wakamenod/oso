//! Each file's project (ADR 0114): asking the thread that finds them, taking in its answers,
//! and the record of trusted projects, which another thread reads and writes.
//!
//! Must not depend on: key presses or how a frame is drawn. Projects are found, and the record
//! read and written, on their own threads, never here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use super::{App, UserEvent, Waker};
use crate::config::{Record, TrustFile};
use crate::loader::{Home, Reader};
use crate::project::{Detector, Trusted};

/// The thread that finds projects, waking the event loop with [`UserEvent::Projects`]. It never
/// takes the home directory for a project.
pub(super) fn detector(waker: &Waker) -> Detector {
    let waker = Mutex::new(waker.clone());
    Detector::new(super::home(), move || {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::Projects)
    })
}

/// The thread that reads EditorConfig after `C-x C-w`, waking the event loop with
/// [`UserEvent::EditorConfig`].
pub(super) fn editorconfig_reader(waker: &Waker, home: &Home) -> Reader {
    let waker = Mutex::new(waker.clone());
    Reader::new(home.clone(), move || {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::EditorConfig)
    })
}

impl App {
    /// Takes the thread that reads the record of trusted projects, and has it wake the event
    /// loop when it finds something new. Called once, before the window opens.
    pub fn set_trust_file(&mut self, trust_file: Option<TrustFile>) {
        if let Some(trust_file) = &trust_file {
            let waker = self.waker.clone();
            trust_file.set_wake(move || {
                let _ = waker.send(UserEvent::Trust);
            });
        }
        self.trust_file = trust_file;
    }

    /// Hands what a person changed of the record of trusted projects to its thread, to make in
    /// the file. With no file, nothing could be changed: the commands refuse while none is read.
    pub(super) fn write_trust(&mut self) {
        let changes = self.editor.take_trust_changes();
        if let Some(trust_file) = &self.trust_file {
            trust_file.change(changes);
        }
    }

    /// oso is going: hands over what a person changed of the record, and waits a little for it
    /// to be made, as `untrust-project` already said it was done.
    pub(super) fn finish_trust(&mut self) {
        self.write_trust();
        if let Some(trust_file) = &self.trust_file {
            trust_file.finish(std::time::Duration::from_secs(1));
        }
    }

    /// Hands the files whose project is wanted to the thread that finds them, with the roots
    /// trusted now.
    pub(super) fn find_projects(&mut self) {
        let requests = self.editor.take_project_requests();
        if requests.is_empty() {
            return;
        }
        let trusted: Trusted = self
            .editor
            .trust_list()
            .map(|list| list.paths().map(PathBuf::from).collect())
            .unwrap_or_else(|| Arc::from([]));
        for request in requests {
            self.projects.find(request, Arc::clone(&trusted));
        }
    }

    /// Starts reading the EditorConfig of the files `C-x C-w` named, each on a thread of its
    /// own (ADR 0115).
    pub(super) fn read_editorconfigs(&mut self) {
        for request in self.editor.take_editorconfig_requests() {
            self.editorconfigs.read(request);
        }
    }

    /// Threads read EditorConfig: gives what they found to their buffers, if their files are
    /// still where they were when asked, and says what was skipped in `*Messages*`.
    pub(super) fn editorconfigs_read(&mut self) {
        for (request, found) in self.editorconfigs.take() {
            let current = self
                .editor
                .buffer(request.buffer)
                .and_then(|buffer| buffer.baseline())
                .is_some_and(|baseline| baseline.location == request.location);
            if !current {
                continue;
            }
            for note in found.notes {
                self.editor.note(note);
            }
            self.editor.set_editorconfig(request.buffer, found.applied);
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

    /// The record of trusted projects' thread found something: gives the record to the editor,
    /// says what it said, and asks for a frame, as the modeline may say `restricted`.
    pub(super) fn trust_reported(&mut self) {
        let Some(look) = self.trust_file.as_ref().and_then(TrustFile::take) else {
            return;
        };
        if let Some(record) = look.record {
            self.editor.set_trust_list(match record {
                Record::Read(list) => Some(list),
                Record::Refused => None,
            });
        }
        for said in look.said {
            self.editor.message(said);
        }
        self.request_redraw();
    }
}
