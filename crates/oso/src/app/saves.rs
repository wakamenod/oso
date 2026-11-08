//! Saving files and leaving: starting the saves commands asked for, taking in how they
//! ended, the window's close button, and ending the event loop once saves have finished. An
//! end nobody here agreed to is the OS's, and writes emergency copies (ADR 0062).
//!
//! Must not depend on: key presses or how a frame is drawn. Files are written on the
//! saver's threads, never here.

use std::time::Instant;

use super::shell::Loop;
use super::{App, UserEvent};
use crate::keys::Held;
use crate::saver::{self, AfterSave, SaveReport};

impl App {
    /// Starts a thread for each save a command queued. Each ends in a [`UserEvent::Save`].
    pub(super) fn start_saves(&mut self) {
        for job in self.editor.take_save_jobs() {
            let started = Instant::now();
            let (buffer, path) = (job.buffer, job.path.clone());
            let waker = self.waker.clone();
            let spawned = saver::spawn(job, move |report| {
                // It fails only once the event loop is gone, past the time limit.
                let _ = waker.send(UserEvent::Save(report));
            });
            log::trace!("save started in {:?}", started.elapsed());
            if let Err(e) = spawned {
                saver::apply(&mut self.editor, SaveReport::not_started(buffer, path, e));
                self.dispatcher.publish_for_emergency(&self.editor);
            }
        }
    }

    /// A saving thread has ended: takes in how it went. If an exit is under way, that may
    /// mean going, or coming back.
    pub(super) fn save_ended(&mut self, event_loop: &Loop<'_>, report: SaveReport) {
        let failed = report.failed();
        let (buffer, confirmed) = (report.buffer(), report.reached_the_disk());
        saver::apply(&mut self.editor, report);
        self.dispatcher.publish_for_emergency(&self.editor);
        self.copies_after_a_save(buffer, confirmed);
        match self.leaving.save_ended(&self.editor, failed) {
            None | Some(AfterSave::KeepWaiting) => {}
            Some(AfterSave::Exit) => self.go(event_loop),
            Some(AfterSave::StayOpen) => {
                self.editor
                    .message("The exit was taken back, as a buffer differs from what is on disk");
                self.stay_open(event_loop);
            }
        }
    }

    /// The window's close button, guarded as `C-x C-c` is, through the same command
    /// (ADR 0059, ADR 0072).
    pub(super) fn close_requested(&mut self, event_loop: &Loop<'_>) {
        let effects = self.dispatcher.close_requested(&mut self.editor);
        if effects.exit {
            self.leave(event_loop, false);
        } else {
            self.request_redraw();
        }
    }

    /// Exits: at once, or if a save is running, once it has ended. A save the user asked
    /// for should not depend on how fast they leave, and a process that ends mid-write
    /// would leave its temporary file behind. The event loop keeps running until then, so
    /// that the window really goes and the process keeps answering the window system.
    ///
    /// `anyway` is for the socket's `exit`, which means to go whatever the buffers hold.
    pub(super) fn leave(&mut self, event_loop: &Loop<'_>, anyway: bool) {
        let Some(deadline) = self.leaving.begin(&self.editor, anyway, Instant::now()) else {
            self.go(event_loop);
            return;
        };
        event_loop.wake_at(deadline);
        if let Some(view) = &self.view {
            view.window.set_visible(false);
        }
    }

    /// Ends the event loop, as this instance has agreed to, so that nothing is written.
    fn go(&mut self, event_loop: &Loop<'_>) {
        self.leaving.go();
        event_loop.exit();
    }

    /// The event loop is ending. Unless this instance agreed to that, it is the OS's doing,
    /// and the process goes with this call, so what is at risk is written here. A save
    /// running now may or may not finish, and nobody can tell, so its buffer gets a copy too.
    pub(super) fn ending(&self) {
        self.emergency.on_exit();
    }

    /// Whether this instance has agreed to go. Nothing that could still be lost with the
    /// process may be accepted from then on.
    pub(super) fn is_leaving(&self, event_loop: &Loop<'_>) -> bool {
        event_loop.exiting() || self.leaving.is_under_way()
    }

    /// Acts on a save that has held an exit for too long.
    pub(super) fn check_leaving(&mut self, event_loop: &Loop<'_>) {
        match self.leaving.check_deadline(&self.editor, Instant::now()) {
            None | Some(AfterSave::KeepWaiting) => {}
            Some(AfterSave::Exit) => {
                log::error!(
                    "leaving with a save that has not finished. Its file may or may not be replaced, and a .oso-save-*.tmp may remain"
                );
                self.go(event_loop);
            }
            Some(AfterSave::StayOpen) => {
                self.editor.reset_exit_guard();
                self.editor.message(
                    "A save has not finished after 10 minutes. The buffer may not be on disk. Exiting again will not wait for it",
                );
                self.stay_open(event_loop);
            }
        }
    }

    /// Takes a leave back: the window returns, with the message that says why.
    fn stay_open(&mut self, event_loop: &Loop<'_>) {
        event_loop.sleep_when_idle();
        // The key that confirmed the exit may have held Control. Its release, if it reached
        // the hidden window at all, may be gone, and the system says again when one is held.
        self.modifiers = Held::NONE;
        if let Some(view) = &self.view {
            // It may be minutes later, with the person typing elsewhere, perhaps a password.
            // So this asks for attention, and does not take the keyboard from another app.
            view.window.set_visible(true);
            view.window.request_attention();
        }
        self.request_redraw();
    }
}
