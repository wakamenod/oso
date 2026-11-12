//! Agent sessions in the app (ADR 0121): the sessions worker, what commands ask of it, the
//! rules the configuration's thread compiled, and what the worker found, carried out on the UI
//! thread. Nothing here reads a file or judges a screen.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::sync::{Arc, Mutex, PoisonError};

use oso_core::buffer::Mode;
use oso_core::editor::SessionRequest;

use super::{App, UserEvent, Waker};
use crate::config::RulesReport;
use crate::sessions::explain::BUFFER;
use crate::sessions::worker::{Job, Out, Worker};

/// The sessions worker for the app, waking the event loop with [`UserEvent::Sessions`]. `None`
/// if it cannot start, which it logs: sessions then have no states.
pub(super) fn worker(waker: &Waker) -> Option<Worker> {
    let waker = Mutex::new(waker.clone());
    Worker::start(move |out| {
        waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(UserEvent::Sessions(out));
    })
}

impl App {
    /// After each event: hands what commands asked about sessions to the worker.
    pub(super) fn sync_sessions(&mut self) {
        for request in self.editor.take_session_requests() {
            let job = match request {
                SessionRequest::ExplainFile(path) => Job::ExplainFile(path),
            };
            match &self.sessions {
                Some(worker) => worker.send(job),
                None => self
                    .editor
                    .message("Sessions are not available: see the log"),
            }
        }
    }

    /// The configuration's thread read the agent rules file: what it says goes to
    /// `*Messages*`, and its rules to the worker, for every session at once (ADR 0121, Q7).
    pub(super) fn agent_rules_reported(&mut self, report: RulesReport) {
        for note in report.notes {
            self.editor.note(note);
        }
        if let (Some(rules), Some(worker)) = (report.rules, &self.sessions) {
            worker.send(Job::Rules(Arc::new(rules)));
        }
    }

    /// The worker found something.
    pub(super) fn sessions_reported(&mut self, out: Out) {
        match out {
            Out::Explained(text) => self.show_explanation(&text),
            Out::Unexplained(why) => self.editor.message(why),
            Out::Slow(slowest) => self.editor.note(format!(
                "Agent rules took more than {} ms on a screen, so its state holds; the slowest \
                 rule was {slowest}. See agent-rules/claude.toml",
                crate::sessions::rules::BUDGET.as_millis()
            )),
            Out::Changed(key, change) => {
                log::debug!("session {key}: {}", change.state);
            }
        }
    }

    /// Shows `text` in `*Session explain*`, which the agent channel cannot read: it holds a
    /// session's screen.
    fn show_explanation(&mut self, text: &str) {
        let id = match self.editor.buffer_named(BUFFER) {
            Some(id) => id,
            None => self.editor.add_internal_buffer(BUFFER, Mode::Fundamental),
        };
        if let Some(buffer) = self.editor.buffer_mut(id) {
            buffer.withhold();
        }
        self.editor.set_internal_text(id, text);
        self.editor.show(id);
        self.request_redraw();
    }
}
