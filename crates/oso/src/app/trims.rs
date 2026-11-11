//! Whitespace cleanup's scans (ADR 0113): each past the budget runs on a thread of its own,
//! as a save does, and what waited for it, a save or the command, goes on when it ends.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here logs what a scan
//! found: its offsets can say something of a buffer that may hold secrets.

use std::thread;

use oso_core::editor::{SaveRefused, TrimReport};

use super::{App, UserEvent};

impl App {
    /// Starts a thread for each scan a command or a report left. Each ends in a
    /// [`UserEvent::Trim`].
    pub(super) fn start_trims(&mut self) {
        for job in self.editor.take_trim_jobs() {
            let buffer = job.buffer();
            let waker = self.waker.clone();
            let spawned = thread::Builder::new()
                .name("oso-trim".into())
                .spawn(move || {
                    // It fails only once the event loop is gone.
                    let _ = waker.send(UserEvent::Trim(job.run()));
                });
            if spawned.is_err() {
                log::warn!("a whitespace scan's thread could not start");
                self.editor.trim_failed(buffer);
                self.agent_save_not_started(
                    buffer,
                    "not saved: the whitespace scan could not start",
                );
                self.request_redraw();
            }
        }
    }

    /// A scan ended: what waited for it goes on, and a save it queued starts, as the agent's
    /// if the agent's command waited. The agent's reply then waits for that save's end, as
    /// for any save of its own, or says at once why there is none: it never hears "saved"
    /// before the file is written.
    pub(super) fn trim_reported(&mut self, report: TrimReport) {
        if let Some(went) = self.editor.trim_reported(report) {
            let not_started = match went.refused {
                Some(refused) => Some(agent_refusal(refused)),
                None => self.start_saves_after_trim(went.for_agent),
            };
            if let Some(why) = not_started {
                self.agent_save_not_started(went.buffer, why);
            }
            self.dispatcher.publish_for_emergency(&self.editor);
            self.request_redraw();
        }
        self.start_trims();
    }
}

/// What the agent hears of a save that could not go on after its scan.
fn agent_refusal(refused: SaveRefused) -> &'static str {
    match refused {
        SaveRefused::ChangedOnDisk => "not saved: the file changed on disk since it was read",
        _ => "not saved; the person's echo area says why",
    }
}
