//! A hosted session's start order, given to its start thread, unless it waits for the
//! configuration's first look (ADR 0173): a resume's or an adoption's, whose buffer is made at
//! once. One that waited takes `[sessions]` and `[terminal]` as they are in force
//! when it goes, not as they were when it was made.
//!
//! Must not depend on: key presses or how a frame is drawn.

use crate::app::{App, UserEvent};
use crate::sessions::start::{self, Order, Report, Runs};

impl App {
    /// Gives `order` to a start thread of its own, or keeps it until the configuration's
    /// first look.
    pub(super) fn order_session(&mut self, order: Order) {
        if self.starts_wait() {
            return self.hold_start(move |app| app.order_held_session(order));
        }
        let waker = std::sync::Mutex::new(self.waker.clone());
        start::spawn(order, move |report| {
            waker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .send(UserEvent::SessionStart(report));
        });
    }

    /// Gives `order`, which waited, to its start thread, with the settings in force now;
    /// nothing if its session was forgotten meanwhile, and refused if its buffer was killed.
    fn order_held_session(&mut self, mut order: Order) {
        if !self.hosting.starting.contains_key(&order.token) {
            self.hosting.shared.take_under_way(order.token);
            return;
        }
        let buffer = match &order.runs {
            Runs::Terminal(terminal) => terminal.buffer,
            Runs::Native(native) => native.buffer,
        };
        let judged = match self.editor.buffer(buffer) {
            None => Err(format!(
                "Session {}'s buffer was killed before it started",
                order.name
            )),
            Some(_) => self.judge_held_order(&mut order),
        };
        if let Err(why) = judged {
            let token = order.token;
            return self.session_start_reported(Report::Refused { token, why });
        }
        order.claude = self.configuration.session_claude().map(Into::into);
        order.steer_worktrees = self.configuration.steer_worktrees();
        let (settings, env) = (self.terminal_settings(), self.configuration.session_env());
        match &mut order.runs {
            Runs::Terminal(terminal) => {
                terminal.settings = settings;
                terminal.env = env.to_vec();
            }
            Runs::Native(native) => {
                native.settings = settings;
                native.env = env.to_vec();
                self.native_control_from_now(order.token);
            }
        }
        self.order_session(order);
    }

    /// Judges `order`, which waited, again as a resume typed now would be: a native one's
    /// trust, and an orchestrator's space with its repositories as they are trusted now.
    fn judge_held_order(&mut self, order: &mut Order) -> Result<(), String> {
        if matches!(order.runs, Runs::Native(_)) {
            let (name, directory) = (&order.name, &order.directory);
            let trusted = self.native_resume_trust(name, directory, order.project.as_deref())?;
            order.trusted = Some(trusted);
        }
        if order.space.is_some() {
            order.space = self.orchestrator_start_for(order.token)?;
        }
        Ok(())
    }
}
