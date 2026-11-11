//! Saving files and leaving: starting the saves commands asked for, taking in how they
//! ended, the window's close button, the OS's requests to end (ADR 0084), and ending the
//! event loop once saves have finished. An end nobody here agreed to is the OS's, and writes
//! emergency copies (ADR 0062).
//!
//! Must not depend on: key presses or how a frame is drawn. Files are written on the
//! saver's threads, never here.

use std::time::Instant;

use oso_core::buffer::BufferId;

use super::shell::Loop;
use super::{App, UserEvent};
use crate::saver::{self, AfterSave, SaveReport};
use oso_platform::window::Held;
#[cfg(target_os = "macos")]
use oso_platform::window::{Event, QuitAnswer, QuitReason};

impl App {
    /// Starts a thread for each save a command queued. Each ends in a [`UserEvent::Save`].
    /// What the person's `y` to a question the agent caused queued is the agent's.
    pub(super) fn start_saves(&mut self) {
        let for_agent = self.dispatcher.take_ran_for_agent();
        let _ = self.spawn_saves(for_agent);
    }

    /// Starts the saves a command run for the agent channel queued. Each is checked on its
    /// thread before it writes, and never goes into oso's configuration directory (ADR 0108).
    /// Returns the buffers whose saves started, and what the agent hears of one that could not.
    pub(super) fn start_agent_saves(&mut self) -> (Vec<BufferId>, Option<&'static str>) {
        self.spawn_saves(true)
    }

    fn spawn_saves(&mut self, for_agent: bool) -> (Vec<BufferId>, Option<&'static str>) {
        let mut started_saves = Vec::new();
        let mut failed = None;
        let configuration = self.configuration_directories();
        for job in self.editor.take_save_jobs() {
            let started = Instant::now();
            let (buffer, path) = (job.buffer, job.path.clone());
            let waker = self.waker.clone();
            // It fails only once the event loop is gone, past the time limit.
            let report = move |report| {
                let _ = waker.send(UserEvent::Save(report));
            };
            let spawned = if for_agent {
                saver::spawn_for_agent(job, configuration.clone(), report)
            } else {
                saver::spawn(job, report)
            };
            log::trace!("save started in {:?}", started.elapsed());
            match spawned {
                Ok(()) => started_saves.push(buffer),
                Err(e) => {
                    let report = SaveReport::not_started(buffer, path, e);
                    failed = failed.or_else(|| report.agent_failure());
                    saver::apply(&mut self.editor, report);
                    self.dispatcher.publish_for_emergency(&self.editor);
                }
            }
        }
        (started_saves, failed)
    }

    /// A saving thread has ended: takes in how it went. If an exit is under way, that may
    /// mean going, or coming back.
    pub(super) fn save_ended(&mut self, event_loop: &Loop<'_>, report: SaveReport) {
        let failed = report.failed();
        let (buffer, confirmed) = (report.buffer(), report.reached_the_disk());
        self.agent_save_ended(&report);
        saver::apply(&mut self.editor, report);
        self.look_again_after_save(buffer);
        // The save may have been of the configuration file, which is read again at once.
        self.look_at_configuration_again();
        self.dispatcher.publish_for_emergency(&self.editor);
        self.copies_after_a_save(buffer, confirmed);
        self.answer_session_end(event_loop);
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

    /// The OS asks to end oso (ADR 0070, ADR 0084). A person's request, from the Dock or with
    /// no reason, is guarded as `C-x C-c` is: it becomes oso's own close request, which may
    /// ask about modified buffers or wait for saves, and the OS is told no. The end of the
    /// session is never refused: it waits a few seconds for running saves, then the answer
    /// is yes, and [`App::ending`] writes copies of what is at risk.
    #[cfg(target_os = "macos")]
    pub(super) fn quit_requested(
        &mut self,
        event_loop: &Loop<'_>,
        reason: QuitReason,
    ) -> QuitAnswer {
        match reason {
            // As the window's close button: through the same path, which takes nothing once
            // an exit is under way.
            QuitReason::Person => {
                self.handle_window_event(event_loop, Event::CloseRequested);
                // With nothing to ask about or wait for, the close request has ended oso: yes.
                if event_loop.exiting() {
                    QuitAnswer::Now
                } else {
                    QuitAnswer::Cancel
                }
            }
            QuitReason::SessionEnd => {
                match self.leaving.begin_session_end(&self.editor, Instant::now()) {
                    Some(_) => {
                        self.set_wake(event_loop);
                        QuitAnswer::Later
                    }
                    None => QuitAnswer::Now,
                }
            }
        }
    }

    /// Lets the end of the session go once its saves have ended or its time is up. On winit
    /// none waits, as winit never asks, so this does nothing there.
    fn answer_session_end(&mut self, event_loop: &Loop<'_>) {
        if self
            .leaving
            .take_session_end_if_ready(&self.editor, Instant::now())
        {
            event_loop.answer_quit(true);
            self.set_wake(event_loop);
        }
    }

    /// Exits: at once, or if a save is running, once it has ended. A save the user asked
    /// for should not depend on how fast they leave, and a process that ends mid-write
    /// would leave its temporary file behind. The event loop keeps running until then, so
    /// that the window really goes and the process keeps answering the window system.
    ///
    /// `anyway` is for the socket's `exit`, which means to go whatever the buffers hold.
    pub(super) fn leave(&mut self, event_loop: &Loop<'_>, anyway: bool) {
        if self
            .leaving
            .begin(&self.editor, anyway, Instant::now())
            .is_none()
        {
            self.go(event_loop);
            return;
        }
        self.set_wake(event_loop);
        self.end_flash();
        if let Some(view) = &self.view {
            view.window.set_visible(false);
        }
    }

    /// Ends the event loop, as this instance has agreed to, so that nothing is written.
    fn go(&mut self, event_loop: &Loop<'_>) {
        // An end of the session the OS holds for an answer goes with this one.
        if self.leaving.take_session_end() {
            event_loop.answer_quit(true);
        }
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
        self.answer_session_end(event_loop);
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
        self.set_wake(event_loop);
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
