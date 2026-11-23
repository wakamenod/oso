//! `oso-restart` on the UI thread (ADR 0133): in the instance going, checking that it may hand
//! over, stopping the agent channel and starting the restart's thread, then answering what that
//! thread asks; in the new instance, starting the thread that reads the pair once the window is
//! up, and answering what it asks. On focus, a new binary in place is looked for, off the UI
//! thread, and said once (step 4). Both answer with the same handover code as `oso dev`'s
//! restart (`handover`, `terminals/carry.rs`, `terminals/adopt.rs`), as the parent.
//!
//! Must not depend on: sockets or processes; the restart's threads hold the pair and the
//! child.

mod wait;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::idle_quit::IdleClock;
use super::shell::Loop;
use super::{App, UserEvent};
use crate::carry::natives::NativeResume;
use crate::carry::{self, Asked, Passed};
use crate::sessions::worker::SessionKey;

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
    /// Since when every native session has been idle, while a restart waits for them (ADR
    /// 0165, part 4b).
    pub(super) wait: IdleClock,
    /// The native sessions a restart ends before its thread starts, until each has (part 4c).
    ending: Option<Ending>,
    /// The native sessions this instance ended for the restart under way, which the state
    /// resumes in the new instance, or which resume here if it stays (part 4c).
    ended: Vec<(SessionKey, NativeResume)>,
}

/// A restart that waits for the native sessions it ended to end, before its thread starts.
struct Ending {
    /// Those that have not ended yet.
    waiting: HashSet<SessionKey>,
    /// When the restart stays if one has still not ended.
    by: Instant,
    /// When they were stopped, for the log.
    since: Instant,
    /// What the restart's thread starts, once they have ended.
    start: carry::going::Start,
}

/// How long the native sessions a restart ends may take: past the stop's `SIGKILL` at 5 s.
const ENDING_TIME: Duration = Duration::from_secs(10);

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
        let holds = self.restart_holds();
        if holds && self.restart.channel_wanted != Some(Wanted::Person) {
            self.restart.channel_wanted = Some(wanted);
        }
        holds
    }

    /// Whether this instance is going: a restart under way here, ending the native sessions
    /// first or not. Nothing resumes then (ADR 0165, part 4c).
    pub(in crate::app) fn restart_going(&self) -> bool {
        self.restart.under_way || self.restart.ending.is_some()
    }

    /// Whether a restart is under way here, ending the native sessions first or not, or this
    /// instance still takes over.
    pub(in crate::app) fn restart_holds(&self) -> bool {
        self.restart_going() || self.restart.taking_over
    }

    /// Starts the agent channel if it was asked for while a restart held it off.
    fn start_the_channel_held_off(&mut self) {
        match self.restart.channel_wanted.take() {
            Some(Wanted::Person) => self.start_agent_channel(),
            Some(Wanted::Session) => self.turn_agent_channel_on_for_a_session(),
            None => {}
        }
    }

    /// `oso-restart`: starts the restart's thread, if this instance may hand over, or waits
    /// for the native sessions to be idle first (ADR 0165, part 4b).
    pub(super) fn begin_restart(&mut self) {
        let refused = if self.restart_going() {
            Err("a restart is under way".to_owned())
        } else {
            self.check_handover(true, true).map(drop)
        };
        let Some(start) = self.restart.start.clone().filter(|_| refused.is_ok()) else {
            let why = refused
                .err()
                .unwrap_or_else(|| "this instance cannot restart".into());
            self.editor.set_restart_when_idle(false);
            self.editor.message(format!("oso-restart: {why}"));
            self.request_redraw();
            return;
        };
        // The restart replaces an exit that waits, which would end what it carries.
        if self.editor.quit_when_idle() {
            self.editor.set_quit_when_idle(false);
            self.editor.reset_exit_guard();
        }
        if self.wait_to_restart() {
            self.request_redraw();
            return;
        }
        self.editor.set_restart_when_idle(false);
        self.restart.channel_was_on = self.agent_channel_on();
        // An agent must not see or steer the restart (ADR 0133, T-agent).
        self.stop_agent_channel();
        // The native sessions that run end first, and the new instance resumes them
        // (ADR 0165, part 4c).
        self.restart.ended = self.end_natives_for_restart();
        if !self.restart.ended.is_empty() {
            self.restart.ending = Some(Ending {
                waiting: self.restart.ended.iter().map(|(key, _)| *key).collect(),
                by: Instant::now() + ENDING_TIME,
                since: Instant::now(),
                start,
            });
            self.editor
                .message("Restarting: the native sessions stop, to resume in the new oso…");
            self.request_redraw();
            return;
        }
        self.start_the_restart(start);
        self.request_redraw();
    }

    /// Native session `key` ended: once each the restart ends has, the restart's checks run
    /// again, as the person may have written meanwhile, and its thread starts.
    pub(in crate::app) fn native_ended_for_restart(&mut self, key: SessionKey) {
        let Some(ending) = &mut self.restart.ending else {
            return;
        };
        // An instance that is going starts no restart, and resumes nothing.
        if self.leaving.is_under_way() {
            self.restart.ending = None;
            return;
        }
        ending.waiting.remove(&key);
        if !ending.waiting.is_empty() {
            return;
        }
        let Some(ending) = self.restart.ending.take() else {
            return;
        };
        log::info!(
            "oso-restart: the native sessions ended in {:?}",
            ending.since.elapsed()
        );
        // A session started or resumed meanwhile would be a second writer once the new oso
        // resumes it, or would be killed part way.
        let checked = match self.native_runs_or_starts() {
            true => Err("a session started while the native sessions ended".to_owned()),
            false => self.check_handover(true, true).map(drop),
        };
        match checked {
            Ok(()) => self.start_the_restart(ending.start),
            Err(why) => self.restart_stays(why),
        }
        self.request_redraw();
    }

    /// Before the loop waits: a restart whose native sessions have not all ended in
    /// [`ENDING_TIME`] stays here.
    pub(in crate::app) fn restart_ending_due(&mut self) {
        let Some(ending) = &self.restart.ending else {
            return;
        };
        if Instant::now() < ending.by {
            return;
        }
        let Some(ending) = self.restart.ending.take() else {
            return;
        };
        // Those still ending resume nothing: their end is a stop, and the person resumes them.
        let waiting = ending.waiting;
        self.restart.ended.retain(|(key, _)| !waiting.contains(key));
        for key in waiting {
            self.native_still_ending(key);
        }
        self.restart_stays(format!(
            "a native session did not end in {} s",
            ENDING_TIME.as_secs()
        ));
        self.request_redraw();
    }

    /// When the loop wakes for a restart's native sessions to have ended.
    pub(in crate::app) fn restart_ending_deadline(&self) -> Option<Instant> {
        self.restart.ending.as_ref().map(|ending| ending.by)
    }

    /// The native sessions the state resumes in the new instance: those ended for this
    /// restart, or, under `oso dev`'s order, which ends none first, those that run still,
    /// which the exit ends.
    pub(in crate::app) fn native_resumes(&self) -> Vec<NativeResume> {
        let mut resumes: Vec<NativeResume> = (self.restart.ended.iter())
            .map(|(_, resume)| resume.clone())
            .collect();
        if self.restart.under_way {
            return resumes;
        }
        for running in self.running_native_resumes() {
            if resumes.iter().all(|resume| resume.id != running.id) {
                resumes.push(running);
            }
        }
        resumes
    }

    /// Starts the restart's thread.
    fn start_the_restart(&mut self, start: carry::going::Start) {
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
        // Before the channel comes back: an agent sees them resuming, never stopped by this.
        let ended = std::mem::take(&mut self.restart.ended);
        self.resume_natives_ended(ended.into_iter().map(|(key, _)| key).collect());
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
        // The native sessions it named resume there (ADR 0165, part 4c).
        self.natives_before_gone(false);
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
