//! Remembering the hosted sessions and the spaces (ADR 0121, ADR 0136): what changed is handed
//! to the sessions worker after each event, and an exit, or a restart's start, waits for its
//! write ("The record at an exit, as built"). As oso starts, the sessions a removed worktree
//! left behind are archived before they come back (ADR 0121, "Removal archives its
//! sessions").
//!
//! Must not depend on: key presses or how a frame is drawn; the worker writes the files.

use crate::app::App;
use crate::sessions::identity::SessionId;
use crate::sessions::record::Remembered;
use crate::sessions::worker::Job;

/// How long an exit, or a restart's start, waits for the sessions worker to write the records:
/// a write takes some milliseconds; a worker that is stuck must not hold the exit.
const FLUSH_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

impl App {
    /// Archives the `remembered` sessions the worker found `stranded`, in a directory gone
    /// under a repository's main checkout (ADR 0121, "Removal archives its sessions"), before
    /// they come back; not one carried over a restart, which runs. Their images stay, to go
    /// with the week's sweep: the disk may only have changed under them. `*Messages*` says how
    /// many. Gives that count.
    pub(super) fn archive_stranded(
        &mut self,
        remembered: &mut [Remembered],
        stranded: &[SessionId],
    ) -> usize {
        if stranded.is_empty() {
            return 0;
        }
        let mut archived = 0;
        for session in remembered.iter_mut() {
            if session.archived || !stranded.contains(&session.id) {
                continue;
            }
            let held = self.hosting.table.iter().any(|held| held.id == session.id);
            if !held {
                session.archived = true;
                archived += 1;
            }
        }
        let count = archived;
        if count > 0 {
            self.editor.message(format!(
                "Archived {}: {} gone from under a repository",
                super::super::worktrees::sessions_counted(count),
                if count == 1 {
                    "its directory is"
                } else {
                    "their directories are"
                },
            ));
        }
        count
    }

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
