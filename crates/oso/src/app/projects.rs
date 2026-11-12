//! Each file's project (ADR 0114): asking the thread that finds them, taking in its answers,
//! and the record of trusted projects, which another thread reads and writes.
//!
//! Must not depend on: key presses or how a frame is drawn. Projects are found, and the record
//! read and written, on their own threads, never here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use super::{App, UserEvent, Waker};
use crate::config::{Record, TrustFile};
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
