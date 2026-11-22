//! `oso-restart` on the UI thread (ADR 0133): in the instance going, checking that it may hand
//! over, stopping the agent channel and starting the restart's thread, then answering what that
//! thread asks; in the new instance, starting the thread that reads the pair once the window is
//! up, and answering what it asks. On focus, a new binary in place is looked for, off the UI
//! thread, and said once (step 4). Both answer with the same handover code as `oso dev`'s
//! restart (`handover`, `terminals/carry.rs`, `terminals/adopt.rs`), as the parent.
//!
//! Must not depend on: sockets or processes; the restart's threads hold the pair and the
//! child.

use super::shell::Loop;
use super::{App, UserEvent};
use crate::carry::{self, Asked, Passed};

/// What a restart keeps on the UI thread.
#[derive(Default)]
pub(super) struct Restart {
    /// The binary and arguments a restart starts, as this instance started.
    pub(super) start: Option<carry::going::Start>,
    /// A restart of this instance is under way.
    under_way: bool,
    /// The agent channel was on when it began, so it comes back if the restart stops short.
    channel_was_on: bool,
    /// In a new instance: the pair from the instance before, until the window is up.
    coming: Option<carry::coming::Inherited>,
    /// In a new instance: the pair is being read.
    taking_over: bool,
    /// Who asked for the agent channel while a restart held it off: it starts once the
    /// restart is over (ADR 0133, T-agent).
    channel_wanted: Option<Wanted>,
    /// Not a restart's, but of the binary it would start (step 4): the thread that looks at
    /// the binary on focus, on Unix, where `oso-restart` runs, and what was said of it.
    looker: Option<carry::notice::Looker>,
    notice: carry::notice::Notice,
}

/// Who asked for the agent channel while a restart held it off.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Wanted {
    /// The person, or the configuration.
    Person,
    /// A hosted session that started or came back.
    Session,
}

impl App {
    /// What this instance starts on `oso-restart`, and, if it takes over from the instance
    /// `parent` through `pair`, those two, until the window is up. With `own`, the identity of
    /// that binary as this instance started, it also starts the thread that looks at it on
    /// focus.
    pub fn set_restart(
        &mut self,
        start: carry::going::Start,
        own: Option<carry::notice::Identity>,
        taking_over: Option<carry::coming::Inherited>,
    ) {
        // `oso-restart` runs on Unix only, so only there is a changed binary worth saying.
        #[cfg(not(unix))]
        let _ = own;
        #[cfg(unix)]
        if let (Some(path), Some(own)) = (start.binary.clone(), own) {
            let waker = self.waker.clone();
            match carry::notice::Looker::start(path, own, move |found| {
                waker.send(UserEvent::BinaryChanged(found));
            }) {
                Ok(looker) => self.restart.looker = Some(looker),
                Err(e) => log::warn!("cannot look for a new binary: {e}"),
            }
        }
        self.restart.start = Some(start);
        self.restart.coming = taking_over;
    }

    /// On focus: looks at the binary this instance came from again, off the UI thread.
    pub(super) fn look_at_binary(&self) {
        if let Some(looker) = &self.restart.looker {
            looker.look_again();
        }
    }

    /// The binary changed: says so once, unless a restart is under way or this
    /// instance still takes over, when a later focus says it.
    pub(super) fn binary_found(&mut self, found: &carry::notice::Found) {
        if self.restart.under_way || self.restart.taking_over || self.restart.coming.is_some() {
            return;
        }
        if let Some(said) = self.restart.notice.say(found) {
            self.editor.message(said);
            self.request_redraw();
        }
    }

    /// After each event: in a new instance whose window is up, starts the thread that reads
    /// the pair.
    pub(super) fn sync_restart(&mut self) {
        if self.view.is_some()
            && let Some(inherited) = self.restart.coming.take()
        {
            self.read_the_pair(inherited);
        }
    }

    fn read_the_pair(&mut self, inherited: carry::coming::Inherited) {
        // A new instance that ends before it speaks, for an end-to-end flow.
        #[cfg(feature = "dev")]
        if dev_fails_at("hello") {
            std::process::exit(3);
        }
        let waker = self.waker.clone();
        match carry::coming::spawn(inherited, move |asked| {
            waker.send(UserEvent::Restart(asked))
        }) {
            Ok(()) => self.restart.taking_over = true,
            Err(e) => log::warn!("oso-restart: cannot read the pair: {e}"),
        }
    }

    /// Whether a restart holds the agent channel off: while one is under way here, or this
    /// instance still takes over. `wanted` is who asked, for when it is over.
    pub(super) fn restart_holds_the_channel(&mut self, wanted: Wanted) -> bool {
        let holds = self.restart.under_way || self.restart.taking_over;
        if holds && self.restart.channel_wanted != Some(Wanted::Person) {
            self.restart.channel_wanted = Some(wanted);
        }
        holds
    }

    /// Whether a restart is under way here, or this instance still takes over.
    pub(in crate::app) fn restart_holds(&self) -> bool {
        self.restart.under_way || self.restart.taking_over
    }

    /// Starts the agent channel if it was asked for while a restart held it off.
    fn start_the_channel_held_off(&mut self) {
        match self.restart.channel_wanted.take() {
            Some(Wanted::Person) => self.start_agent_channel(),
            Some(Wanted::Session) => self.turn_agent_channel_on_for_a_session(),
            None => {}
        }
    }

    /// `oso-restart`: starts the restart's thread, if this instance may hand over.
    pub(super) fn begin_restart(&mut self) {
        let refused = if self.restart.under_way {
            Err("a restart is under way".to_owned())
        } else {
            self.check_handover(true, true).map(drop)
        };
        let Some(start) = self.restart.start.clone().filter(|_| refused.is_ok()) else {
            let why = refused
                .err()
                .unwrap_or_else(|| "this instance cannot restart".into());
            self.editor.message(format!("oso-restart: {why}"));
            self.request_redraw();
            return;
        };
        self.restart.channel_was_on = self.agent_channel_on();
        // An agent must not see or steer the restart (ADR 0133, T-agent).
        self.stop_agent_channel();
        // The new instance reads the sessions, spaces and recent files as it starts, before this
        // one exits.
        self.flush_records();
        let waker = self.waker.clone();
        match carry::going::spawn(start, move |asked| waker.send(UserEvent::Restart(asked))) {
            Ok(()) => {
                self.restart.under_way = true;
                self.editor.message("Restarting…");
            }
            Err(e) => self.restart_stays(format!("cannot start the restart: {e}")),
        }
        self.request_redraw();
    }

    /// What a thread of the restart asks.
    pub(super) fn restart_asked(&mut self, event_loop: &Loop<'_>, asked: Asked) {
        match asked {
            Asked::Op { op, passed, reply } => {
                if self.is_leaving(event_loop) {
                    let _ = reply.send(Err("the editor is shutting down".into()));
                    return;
                }
                self.drop_stale_texts();
                if let Some((reply, result)) = self.answer_restart(op, reply, passed) {
                    let _ = reply.send(result);
                }
            }
            Asked::Exit { generation, reply } => {
                // An exit refused at the last moment, for an end-to-end flow.
                #[cfg(feature = "dev")]
                if dev_fails_at("exit") {
                    let _ = reply.send(Err(carry::EXIT_REFUSED.to_owned()));
                    return;
                }
                let _ = reply.send(self.exit_for_restart(event_loop, Some(generation)));
            }
            Asked::Stays(why) => self.restart_stays(why),
            Asked::PreviousStays { generation } => {
                self.left_by_the_one_before(event_loop, generation);
            }
            Asked::Over => {
                self.restart.taking_over = false;
                self.start_the_channel_held_off();
            }
        }
        self.request_redraw();
    }

    /// The restart is over and this instance stays: its terminals read again, the channel comes
    /// back if it was on, and why is said.
    fn restart_stays(&mut self, why: String) {
        self.restart.under_way = false;
        self.resume_terminals();
        self.forget_export();
        if std::mem::take(&mut self.restart.channel_was_on) {
            self.start_agent_channel();
        }
        self.start_the_channel_held_off();
        self.editor
            .message(format!("oso-restart: {why}; everything stays here"));
    }

    /// In a new instance: the one before stays. Its terminals are given up here, and this one
    /// closes unless it was changed since the import.
    fn left_by_the_one_before(&mut self, event_loop: &Loop<'_>, generation: Option<u64>) {
        let passed = Passed {
            parent: true,
            ..Passed::default()
        };
        if let Err(why) = self.import_terminals(None, passed) {
            log::debug!("oso-restart: the carried terminals were not given up: {why}");
        }
        let allowed = carry::exit_allowed(&self.editor, generation)
            .and_then(|()| self.refuse_unsent_queues());
        if generation.is_some() && allowed.is_ok() {
            self.leave(event_loop, true);
        } else {
            self.editor.message(
                "oso-restart: the oso before stays, and so does this one, as it was changed here",
            );
        }
    }
}

/// Whether a dev build's end-to-end flow asks the restart to stop short at `point`
/// (`OSO_DEV_RESTART_FAIL`): `hello`, the new instance ends before it speaks, or `exit`, the
/// instance going refuses to exit once the new one holds everything.
#[cfg(feature = "dev")]
fn dev_fails_at(point: &str) -> bool {
    std::env::var("OSO_DEV_RESTART_FAIL").is_ok_and(|asked| asked == point)
}
