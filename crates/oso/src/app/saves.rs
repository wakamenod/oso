//! Saving files and leaving: starting the saves commands asked for, taking in how they
//! ended, the window's close button, and ending the event loop once saves have finished.
//!
//! Must not depend on: key presses or how a frame is drawn. Files are written on the
//! saver's threads, never here.

use std::time::{Duration, Instant};

use oso_core::editor::ExitAction;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::ModifiersState;
use winit::window::UserAttentionType;

use super::{App, UserEvent};
use crate::saver::{self, AfterSave, SaveReport};

/// How long a save still running may hold the process once the window is hidden. It
/// is long enough for many gigabytes on a slow disk, and abandoning a save that is
/// getting somewhere is the worse mistake: a process with no window costs little. A
/// save that hangs past it (a stuck network file system) cannot keep it for good.
const FINISH_TIMEOUT: Duration = Duration::from_secs(600);

/// An exit agreed to while a save runs.
#[derive(Clone, Copy)]
pub(super) struct Leaving {
    /// When to stop waiting for the save.
    deadline: Instant,
    /// Go even if the save fails, as the socket's `exit` asks.
    anyway: bool,
}

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

    /// A saving thread has ended: takes in how it went. If this instance is leaving, that
    /// may mean going, or coming back.
    pub(super) fn save_ended(&mut self, event_loop: &ActiveEventLoop, report: SaveReport) {
        let failed = report.failed();
        saver::apply(&mut self.editor, report);
        self.stuck_saves.retain(|id| {
            self.editor
                .buffer(*id)
                .is_some_and(|buffer| buffer.is_saving())
        });
        let still_saving = saver::any_worth_waiting_for(&self.editor, &self.stuck_saves);
        let Some(leaving) = self.leaving else {
            return;
        };
        match saver::after_save_while_leaving(failed, leaving.anyway, still_saving) {
            AfterSave::KeepWaiting => {}
            AfterSave::Exit => event_loop.exit(),
            AfterSave::StayOpen => self.stay_open(event_loop),
        }
    }

    /// The window's close button, guarded as `C-x C-c` is (ADR 0059).
    pub(super) fn close_requested(&mut self, event_loop: &ActiveEventLoop) {
        match self.editor.confirm_exit(ExitAction::CloseWindow) {
            Ok(()) => self.leave(event_loop, false),
            Err(refused) => self.editor.message(refused.to_string()),
        }
    }

    /// Exits: at once, or if a save is running, once it has ended. A save the user asked
    /// for should not depend on how fast they leave, and a process that ends mid-write
    /// would leave its temporary file behind. The event loop keeps running until then, so
    /// that the window really goes and the process keeps answering the window system.
    ///
    /// `anyway` is for the socket's `exit`, which means to go whatever the buffers hold. A
    /// person's exit comes back if the save fails, as the text is then only here.
    pub(super) fn leave(&mut self, event_loop: &ActiveEventLoop, anyway: bool) {
        if !saver::any_worth_waiting_for(&self.editor, &self.stuck_saves) {
            event_loop.exit();
            return;
        }
        let deadline = Instant::now() + FINISH_TIMEOUT;
        self.leaving = Some(Leaving { deadline, anyway });
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

    /// Acts on a save that has held the leaving for too long. Nobody can tell whether the
    /// text reached the disk, so a person gets the window and the buffer back, and exiting
    /// again then goes without waiting. The socket's `exit` goes now.
    pub(super) fn check_leaving(&mut self, event_loop: &ActiveEventLoop) {
        let Some(leaving) = self.leaving else {
            return;
        };
        if Instant::now() < leaving.deadline {
            return;
        }
        match saver::at_leaving_deadline(leaving.anyway) {
            AfterSave::Exit => {
                log::error!(
                    "leaving with a save that has not finished. Its file may or may not be replaced, and a .oso-save-*.tmp may remain"
                );
                event_loop.exit();
            }
            _ => {
                self.stuck_saves = self
                    .editor
                    .buffers()
                    .filter(|(_, buffer)| buffer.is_saving())
                    .map(|(id, _)| id)
                    .collect();
                self.editor.reset_exit_guard();
                self.editor.message(
                    "A save has not finished after 10 minutes. The buffer may not be on disk. Exiting again will not wait for it",
                );
                self.stay_open(event_loop);
            }
        }
    }

    /// Takes a leave back: the window returns, with the message that says why.
    fn stay_open(&mut self, event_loop: &ActiveEventLoop) {
        self.leaving = None;
        event_loop.set_control_flow(ControlFlow::Wait);
        // The key that confirmed the exit may have held Control. Its release, if it reached
        // the hidden window at all, may be gone, and the system says again when one is held.
        self.modifiers = ModifiersState::default();
        if let Some(view) = &self.view {
            // It may be minutes later, with the person typing elsewhere, perhaps a password.
            // So this asks for attention, and does not take the keyboard from another app.
            view.window.set_visible(true);
            view.window
                .request_user_attention(Some(UserAttentionType::Informational));
        }
        self.request_redraw();
    }
}
