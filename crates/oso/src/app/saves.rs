//! Saving files and leaving: starting the saves commands asked for, taking in how they
//! ended, the window's close button, and ending the event loop once saves have finished.
//!
//! Must not depend on: key presses or how a frame is drawn. Files are written on the
//! saver's threads, never here.

use std::time::{Duration, Instant};

use oso_core::editor::ExitAction;
use winit::event_loop::{ActiveEventLoop, ControlFlow};

use super::{App, UserEvent};
use crate::saver::{self, SaveReport};

/// How long a save still running may hold the process once the window is hidden. It
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
                // It fails only once the event loop is gone, past the time limit.
                let _ = proxy.send_event(UserEvent::Save(report));
            });
            log::trace!("save started in {:?}", started.elapsed());
            if let Err(e) = spawned {
                saver::apply(&mut self.editor, SaveReport::not_started(buffer, path, e));
            }
        }
    }

    /// A saving thread has ended: takes in how it went, and ends the event loop if that
    /// was all it was waiting for.
    pub(super) fn save_ended(&mut self, event_loop: &ActiveEventLoop, report: SaveReport) {
        if self.leaving.is_some() {
            report.log_if_failed();
        }
        saver::apply(&mut self.editor, report);
        if self.leaving.is_some() && self.editor.buffers().all(|(_, b)| !b.is_saving()) {
            event_loop.exit();
        }
    }

    /// The window's close button, guarded as `C-x C-c` is (ADR 0059).
    pub(super) fn close_requested(&mut self, event_loop: &ActiveEventLoop) {
        match self.editor.confirm_exit(ExitAction::CloseWindow) {
            Ok(()) => self.leave(event_loop),
            Err(refused) => self.editor.message(refused.to_string()),
        }
    }

    /// Exits: at once, or if a save is running, once it has ended. A save the user asked
    /// for should not depend on how fast they leave, and a process that ends mid-write
    /// would leave its temporary file behind. The event loop keeps running until then, so
    /// that the window really goes and the process keeps answering the window system.
    pub(super) fn leave(&mut self, event_loop: &ActiveEventLoop) {
        if self.editor.buffers().all(|(_, buffer)| !buffer.is_saving()) {
            event_loop.exit();
            return;
        }
        let deadline = Instant::now() + FINISH_TIMEOUT;
        self.leaving = Some(deadline);
        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        if let Some(view) = &self.view {
            view.window.set_visible(false);
        }
    }

    /// Whether this instance has agreed to go. Nothing that could still be lost with the
    /// process may be accepted from then on.
    pub(super) fn is_leaving(&self, event_loop: &ActiveEventLoop) -> bool {
        event_loop.exiting() || self.leaving.is_some()
    }

    /// Ends the event loop if a save has held it for too long.
    pub(super) fn check_leaving(&self, event_loop: &ActiveEventLoop) {
        if self
            .leaving
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            log::error!(
                "leaving with a save that has not finished. Its file may or may not be replaced, and a .oso-save-*.tmp may remain"
            );
            event_loop.exit();
        }
    }
}
