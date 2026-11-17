//! Remembering the hosted sessions and the spaces (ADR 0121, ADR 0136): what changed is handed
//! to the sessions worker after each event, and an exit, or a restart's start, waits for its
//! write ("The record at an exit, as built").
//!
//! Must not depend on: key presses or how a frame is drawn; the worker writes the files.

use crate::app::App;
use crate::sessions::worker::Job;

/// How long an exit, or a restart's start, waits for the sessions worker to write the records:
/// a write takes some milliseconds; a worker that is stuck must not hold the exit.
const FLUSH_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

impl App {
    /// Hands the sessions and spaces to the worker to remember, if they changed, and waits at
    /// most [`FLUSH_WAIT`] until it has written them: before the process goes, or a restart's new
    /// instance reads them (ADR 0121, "The record at an exit, as built"). Past the wait it goes
    /// on, and says so in the log.
    pub(in crate::app) fn flush_sessions(&mut self) {
        self.save_sessions();
        let Some(worker) = &self.sessions else {
            return;
        };
        if !worker.flush(FLUSH_WAIT) {
            log::warn!(
                "the sessions worker did not say within {FLUSH_WAIT:?} that the sessions and spaces are written"
            );
        }
    }

    /// After an event: hands the sessions to the worker to remember, keeping another
    /// instance's, if they changed, and the spaces to keep, if they did (ADR 0136).
    pub(in crate::app) fn save_sessions(&mut self) {
        let (Some(worker), Some(directory)) = (&self.sessions, &self.hosting.directory) else {
            return;
        };
        if std::mem::take(&mut self.hosting.unsaved) {
            worker.send(Job::Remember {
                directory: directory.clone(),
                sessions: self.hosting.table.remembered(),
                known: self.hosting.table.known().clone(),
            });
        }
        if let Some((spaces, known)) = self.watch.spaces_mut().take_unsaved() {
            worker.send(Job::Space(crate::sessions::worker::SpaceJob::Remember {
                directory: directory.clone(),
                spaces,
                known,
            }));
        }
    }
}
