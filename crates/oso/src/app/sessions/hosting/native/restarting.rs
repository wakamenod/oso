//! Native sessions across `oso-restart` (ADR 0165, part 4c, Q8): the instance going ends each
//! native process that runs, and the new instance resumes each in its buffer, in the mode,
//! with the model and effort, it had (Q4-2), saying so in one row (Q4-3). Its rows are not
//! carried.
//!
//! Must not depend on: key presses or how a frame is drawn; the restart's order is the app's
//! restart's, and the state's field is the restart state's.
//!
//! - **Going.** Once the restart's wait is over (part 4b) and its checks pass, each native
//!   session whose `claude -p` runs, and that was given a message, is stopped as the session
//!   list stops it: standard input closed and its group sent `SIGTERM`, `SIGKILL` after 5 s.
//!   Its end is no failure, and nothing resumes of itself while the restart holds. The
//!   restart's thread starts once each has ended. One given no message has no conversation to
//!   resume, and the exit ends it as before.
//!   Nothing resumes here while the restart goes on, and a session started meanwhile keeps
//!   the restart here.
//! - **Staying.** A restart that stays here, for any reason, resumes each session it ended
//!   here, under the trust judged now (part 4a), unless it has not ended yet.
//! - **Coming.** The state names each session by its id, with its mode, model and effort, and
//!   only the parent's state is read. Once the instance before has gone, and the record is
//!   read, each id the record has as a native session, stopped, with its buffer, is resumed
//!   with `session-resume`'s checks: its directory and root are the record's, never the
//!   state's, its trust is judged (part 4a), its start may wait for the configuration's first
//!   look (ADR 0173), and the start thread refuses a conversation live elsewhere. A refusal
//!   leaves it stopped, saying why, in the mode it had (`resumed`). A session the state does not name
//!   stays as it came back: stopped; so does one resumed meanwhile by anyone else.

use std::time::Instant;

use super::resumed::ResumeFor;
use crate::app::App;
use crate::carry::natives::NativeResume;
use crate::sessions::hosted::Run;
use crate::sessions::native::choice::Choice;
use crate::sessions::native::mode::ModeState;
use crate::sessions::worker::SessionKey;

/// What the new instance holds of the native sessions a restart resumes.
#[derive(Debug, Default)]
pub(in crate::app) struct Resuming {
    /// What the state named, until they are resumed.
    resumes: Vec<NativeResume>,
    /// The instance before has gone.
    gone: bool,
    /// Those resumed, until each has reported (`resumed`).
    pub(super) asked: Vec<SessionKey>,
    /// Since when each has reported, while what they left stopped waits to be said.
    pub(super) reported: Option<Instant>,
}

impl App {
    /// The native sessions whose `claude -p` runs and that were given a message: those a
    /// restart ends and resumes. Not one oso is stopping already, or that the person is asked
    /// whether to stop, or an orchestrator being replaced: their stop wins; nor one being
    /// handed to the TUI (part 4d), which the restart's wait holds for.
    fn natives_to_resume(&self) -> impl Iterator<Item = SessionKey> + '_ {
        (self.hosting.natives.map.iter())
            .filter(|(_, native)| native.stream.is_some() && native.spoke)
            .map(|(key, _)| *key)
            .filter(|key| {
                !self.watch.is_hung_up(*key)
                    && !self.watch.stop_asked(*key)
                    && !self.hosting.spaces.replacing.contains_key(key)
                    && !self.hosting.natives.switches.under_way(*key)
            })
    }

    /// The instance going: ends each native session that runs and was given a message, as
    /// the session list's stop does, and returns each, with what the new instance resumes
    /// it with.
    pub(in crate::app) fn end_natives_for_restart(&mut self) -> Vec<(SessionKey, NativeResume)> {
        let running: Vec<SessionKey> = self.natives_to_resume().collect();
        let mut ended = Vec::with_capacity(running.len());
        for key in running {
            let Some(resume) = self.native_resume_of(key) else {
                continue;
            };
            self.hang_up_session(key);
            ended.push((key, resume));
        }
        ended
    }

    /// Whether a native session given a message runs, or any session is starting: once the
    /// restart has ended the native sessions, one started meanwhile keeps it here.
    pub(in crate::app) fn native_runs_or_starts(&self) -> bool {
        self.natives_to_resume().next().is_some() || self.sessions_starting()
    }

    /// What the state says of each native session that runs and was given a message now,
    /// which the exit ends: those `oso dev`'s restart carries, as its order ends none first.
    pub(in crate::app) fn running_native_resumes(&self) -> Vec<NativeResume> {
        (self.natives_to_resume())
            .filter_map(|key| self.native_resume_of(key))
            .collect()
    }

    /// What the state says of native session `key`: its id, and the mode, model and effort
    /// its next process starts with.
    fn native_resume_of(&self, key: SessionKey) -> Option<NativeResume> {
        let session = self.hosting.table.get(key)?;
        let native = self.hosting.natives.map.get(&key)?;
        let (model, effort) = native.choice.for_start();
        Some(NativeResume {
            id: session.id.as_str().to_owned(),
            mode: native.mode.carried(),
            model,
            effort: effort.map(str::to_owned),
        })
    }

    /// The restart stays here: each of `ended`, which it ended, resumes in its buffer, under
    /// the trust judged now, or stays stopped, saying why.
    pub(in crate::app) fn resume_natives_ended(&mut self, ended: Vec<SessionKey>) {
        // Nothing starts in an instance that is going.
        if self.leaving.is_under_way() {
            return;
        }
        for key in ended {
            if !self.hosting.natives.has(key) {
                continue;
            }
            // oso ended it for the restart, so nothing went on elsewhere: no read (part 5b),
            // unless it stays stopped.
            self.native_may_have_gone_on(key, false);
            self.native_resumes_for(key, ResumeFor::Stayed);
            if let Err(why) = self.resume_hosted(key, false) {
                self.native_resume_failed(key, &why);
            }
        }
    }

    /// The restart stays while native session `key` has not ended: it is not resumed, and
    /// says so.
    pub(in crate::app) fn native_still_ending(&mut self, key: SessionKey) {
        self.native_note_row(
            key,
            "oso-restart stayed here before this session ended: session-resume resumes it",
        );
    }

    /// The new instance took over a state that names `resumes`, from its parent: kept until
    /// the instance before has gone.
    pub(in crate::app) fn natives_carried(&mut self, resumes: Vec<NativeResume>) {
        self.hosting.natives.resuming = Resuming {
            resumes,
            ..Resuming::default()
        };
    }

    /// Session `key` is resumed now by another's asking: not again as a restart's.
    pub(in crate::app) fn forget_restart_resume(&mut self, key: SessionKey) {
        let Some(session) = self.hosting.table.get(key) else {
            return;
        };
        let id = session.id.as_str();
        (self.hosting.natives.resuming.resumes).retain(|resume| resume.id != id);
    }

    /// The new instance: the instance before has gone, if `gone`, and the sessions named
    /// resume once the record is read; or it stays, and none does.
    pub(in crate::app) fn natives_before_gone(&mut self, gone: bool) {
        let resuming = &mut self.hosting.natives.resuming;
        if !gone {
            resuming.resumes.clear();
            return;
        }
        resuming.gone = true;
        self.resume_natives_carried();
    }

    /// The new instance: once the instance before has gone and the record is read, resumes
    /// each session the state names that the record has as a stopped native one.
    pub(in crate::app) fn resume_natives_carried(&mut self) {
        let resuming = &self.hosting.natives.resuming;
        if !resuming.gone || resuming.resumes.is_empty() || !self.hosting.adopting.recalled {
            return;
        }
        for resume in std::mem::take(&mut self.hosting.natives.resuming.resumes) {
            let key = (self.hosting.table.iter())
                .find(|session| session.id.as_str() == resume.id)
                .filter(|session| session.native && session.run == Run::Stopped)
                .map(|session| session.key);
            let Some(key) = key.filter(|key| self.hosting.natives.has(*key)) else {
                log::debug!("oso-restart: no stopped native session named so to resume");
                continue;
            };
            self.hosting.natives.resuming.asked.push(key);
            self.resume_native_carried(key, &resume);
        }
    }

    /// Resumes native session `key` as `resume` says; its transcript says so once it runs. The mode,
    /// model and effort carried are the session's whether it resumes or not, as they were
    /// before the restart, and its next resume starts with them: a refusal names the mode.
    fn resume_native_carried(&mut self, key: SessionKey, resume: &NativeResume) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.mode = ModeState::restarted(resume.mode);
            native.choice = Choice::restarted(resume.model.as_deref(), resume.effort.as_deref());
        }
        self.native_resumes_for(key, ResumeFor::Restarted(resume.mode));
        if let Err(why) = self.resume_hosted(key, false) {
            self.native_resume_failed(key, &why);
        }
    }
}
