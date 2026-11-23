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
//! - **Staying.** A restart that stays here, for any reason, resumes each session it ended
//!   here, under the trust judged now (part 4a).
//! - **Coming.** The state names each session by its id, with its mode, model and effort, and
//!   only the parent's state is read. Once the instance before has gone, and the record is
//!   read, each id the record has as a native session, stopped, with its buffer, is resumed
//!   with `session-resume`'s checks: its directory and root are the record's, never the
//!   state's, its trust is judged (part 4a), its start may wait for the configuration's first
//!   look (ADR 0173), and the start thread refuses a conversation live elsewhere. A refusal
//!   leaves it stopped, saying why. A session the state does not name stays as it came back:
//!   stopped.

use super::draw;
use crate::app::App;
use crate::carry::natives::NativeResume;
use crate::sessions::native::choice::Choice;
use crate::sessions::native::mode::ModeState;
use crate::sessions::worker::SessionKey;

/// What the new instance holds of the native sessions a restart resumes.
#[derive(Debug, Default)]
pub(in crate::app) struct Carried {
    /// What the state named, until they are resumed.
    resumes: Vec<NativeResume>,
    /// The instance before has gone.
    gone: bool,
}

/// The row a session the new instance resumes says.
const RESUMED: &str = "Resumed after oso-restart.";

impl App {
    /// The instance going: ends each native session that runs and was given a message, as
    /// the session list's stop does, and returns each, with what the new instance resumes
    /// it with.
    pub(in crate::app) fn end_natives_for_restart(&mut self) -> Vec<(SessionKey, NativeResume)> {
        let running: Vec<SessionKey> = (self.hosting.natives.map.iter())
            .filter(|(_, native)| native.stream.is_some() && native.spoke)
            .map(|(key, _)| *key)
            .collect();
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

    /// What the state says of each native session that runs and was given a message now,
    /// which the exit ends: those `oso dev`'s restart carries, as its order ends none first.
    pub(in crate::app) fn running_native_resumes(&self) -> Vec<NativeResume> {
        (self.hosting.natives.map.iter())
            .filter(|(_, native)| native.stream.is_some() && native.spoke)
            .filter_map(|(key, _)| self.native_resume_of(*key))
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
        for key in ended {
            if !self.hosting.natives.has(key) {
                continue;
            }
            let said = match self.resume_hosted(key, false) {
                Ok(()) => "oso-restart stayed here: resumed.".to_owned(),
                Err(why) => format!("oso-restart stayed here; it did not resume: {why}"),
            };
            self.native_note_row(key, &said);
        }
    }

    /// The new instance took over a state that names `resumes`, from its parent: kept until
    /// the instance before has gone.
    pub(in crate::app) fn natives_carried(&mut self, resumes: Vec<NativeResume>) {
        self.hosting.natives.carried = Carried {
            resumes,
            gone: false,
        };
    }

    /// The new instance: the instance before has gone, if `gone`, and the sessions named
    /// resume once the record is read; or it stays, and none does.
    pub(in crate::app) fn natives_before_gone(&mut self, gone: bool) {
        let carried = &mut self.hosting.natives.carried;
        if !gone {
            carried.resumes.clear();
            return;
        }
        carried.gone = true;
        self.resume_natives_carried();
    }

    /// The new instance: once the instance before has gone and the record is read, resumes
    /// each session the state names that the record has as a stopped native one.
    pub(in crate::app) fn resume_natives_carried(&mut self) {
        let carried = &self.hosting.natives.carried;
        if !carried.gone || carried.resumes.is_empty() || !self.hosting.adopting.recalled {
            return;
        }
        for resume in std::mem::take(&mut self.hosting.natives.carried.resumes) {
            let key = (self.hosting.table.iter())
                .find(|session| session.id.as_str() == resume.id)
                .filter(|session| session.native && !session.runs())
                .map(|session| session.key);
            let Some(key) = key.filter(|key| self.hosting.natives.has(*key)) else {
                log::debug!(
                    "oso-restart: no stopped native session {} to resume",
                    resume.id
                );
                continue;
            };
            self.resume_native_carried(key, &resume);
        }
    }

    /// Resumes native session `key` as `resume` says, saying so in its transcript.
    fn resume_native_carried(&mut self, key: SessionKey, resume: &NativeResume) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.mode = ModeState::restarted(resume.mode);
            native.choice = Choice::restarted(resume.model.as_deref(), resume.effort.as_deref());
        }
        let said = match self.resume_hosted(key, false) {
            Ok(()) => RESUMED.to_owned(),
            Err(why) => format!("It did not resume after oso-restart: {why}"),
        };
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let change = native.rows.note(&said);
        draw(&mut self.editor, native, [change]);
        self.request_redraw();
    }
}
