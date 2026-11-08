//! isearch's workers (ADR 0082): starting a thread for the job a search left, and taking in
//! what it found and counted.
//!
//! Must not depend on: key presses or how a frame is drawn. The search and the count run on
//! the worker's thread over a snapshot, never here.
//!
//! Nothing here logs what a worker reports: its offsets can say something of a buffer that
//! may hold secrets.

use std::thread;

use oso_core::search::SearchReport;

use super::{App, UserEvent};

impl App {
    /// Searches again if the text changed under isearch, and starts a thread for the job
    /// that leaves, if any. A newer job stops the one before within a block of text. Each
    /// part the job ends comes back as a [`UserEvent::Search`].
    pub(super) fn start_searches(&mut self) {
        if self.editor.isearch_fresh() {
            self.request_redraw();
        }
        let Some(job) = self.editor.take_search_job() else {
            return;
        };
        let waker = self.waker.clone();
        let spawned = thread::Builder::new()
            .name("oso-search".into())
            .spawn(move || {
                // It fails only once the event loop is gone. The snapshot is freed here.
                job.run(|report| {
                    let _ = waker.send(UserEvent::Search(report));
                });
            });
        if spawned.is_err() {
            // The search waits for `C-g`, and the count stays unknown.
            log::warn!("a search thread could not start");
        }
    }

    /// A worker found or counted something.
    pub(super) fn search_reported(&mut self, report: SearchReport) {
        if self.editor.search_reported(report) {
            self.request_redraw();
        }
        self.start_searches();
    }
}
