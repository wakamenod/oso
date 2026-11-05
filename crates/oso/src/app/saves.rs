//! Saving files and leaving: starting the saves commands asked for, taking in how they
//! ended, the window's close button, and letting saves finish after the event loop.
//!
//! Must not depend on: key presses or how a frame is drawn. Files are written on the
//! saver's threads, never here.

use std::time::{Duration, Instant};

use oso_core::editor::ExitAction;
use winit::event_loop::ActiveEventLoop;

use super::{App, UserEvent};
use crate::saver::{self, SaveReport};

/// How long a save still running may hold the process once the window is gone. It
/// is long enough for many gigabytes on a slow disk, and abandoning a save that is
/// getting somewhere is the worse mistake: a process with no window costs little. A
/// save that hangs past it (a stuck network file system) cannot keep it for good.
const FINISH_TIMEOUT: Duration = Duration::from_secs(600);

impl App {
    /// Starts a thread for each save a command queued. Each ends in a [`UserEvent::Save`].
    pub(super) fn start_saves(&mut self) {
        for job in self.editor.take_save_jobs() {
            let started = Instant::now();
            let (buffer, path) = (job.buffer, job.path.clone());
            let proxy = self.proxy.clone();
            let spawned = saver::spawn(job, move |report| {
                // After a confirmed exit, nothing is left to tell but the log. `Debug` escapes
                // what a path may hold, and a report holds no text.
                if let Err(e) = proxy.send_event(UserEvent::Save(report)) {
                    log::error!("a save ended after the editor: {:?}", e.0);
                }
            });
            log::trace!("save started in {:?}", started.elapsed());
            match spawned {
                Ok(handle) => self.saves.push(handle),
                Err(e) => saver::apply(&mut self.editor, SaveReport::not_started(buffer, path, e)),
            }
        }
    }

    /// A saving thread has ended: takes in how it went.
    pub(super) fn save_ended(&mut self, report: SaveReport) {
        saver::apply(&mut self.editor, report);
        self.saves.retain(|save| !save.is_finished());
    }

    /// The window's close button, guarded as `C-x C-c` is (ADR 0057).
    pub(super) fn close_requested(&mut self, event_loop: &ActiveEventLoop) {
        match self.editor.confirm_exit(ExitAction::CloseWindow) {
            Ok(()) => event_loop.exit(),
            Err(refused) => self.editor.message(refused.to_string()),
        }
    }

    /// Once the event loop has ended, closes the window and lets the saves still running
    /// finish. A save the user asked for should not depend on how fast they leave, and
    /// a process that ends mid-write would leave its temporary file behind. There is no
    /// UI left here to block.
    pub fn finish_saves(&mut self) {
        self.saves.retain(|save| !save.is_finished());
        if self.saves.is_empty() {
            return;
        }
        self.view = None;
        if !saver::wait_for(&self.saves, FINISH_TIMEOUT) {
            log::error!(
                "leaving with a save that has not finished. Its file may or may not be replaced, and a .oso-save-*.tmp may remain"
            );
        }
    }
}
