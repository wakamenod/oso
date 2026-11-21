//! Replacing a space's orchestrator (ADR 0136, step 5): `space-orchestrator-replace` stops the
//! old one, once it is neither working nor waiting for input, keeps it as an earlier one under
//! `<name>-was-<n>`, and starts the successor under the space's name once the old one ended,
//! told whom it takes over from. A missing `handoff.md` is asked about first.
//!
//! Must not depend on: key presses or how a frame is drawn. Its looks at the disk run on a
//! space probe thread.

use std::path::PathBuf;

use oso_core::minibuffer::PromptId;

use crate::app::App;
use crate::app::sessions::worktrees::hosted_state;
use crate::sessions::identity::SessionId;
use crate::sessions::spaces::SpaceId;
use crate::sessions::spaces::orchestrator::{earlier_name, next_earlier, takeover_brief};
use crate::sessions::spaces::stored::now_ms;
use crate::sessions::state::State;
use crate::sessions::worker::{Job, SessionKey, SpaceJob};

/// A replacement asked about, as `handoff.md` was missing: done on `y` to that question.
#[derive(Debug)]
pub(in crate::app) struct Asked {
    pub(super) prompt: PromptId,
    space: SpaceId,
    transcript: Option<PathBuf>,
}

/// The successor that starts once the old orchestrator ended.
#[derive(Debug)]
pub(in crate::app) struct Successor {
    space: SpaceId,
    brief: String,
}

impl App {
    /// `space-orchestrator-replace`: looks whether `handoff.md` is there, and where the old
    /// orchestrator's transcript is, before anything stops.
    pub(in crate::app) fn replace_orchestrator(&mut self) {
        let Some(at) = self.watch.spaces().current() else {
            return self.editor.message("No space in front".to_owned());
        };
        let (old, space, directory) = match self.replaceable(at) {
            Ok(found) => found,
            Err(why) => return self.editor.message(why),
        };
        self.hosting.spaces.probing.insert(space.clone());
        let job = SpaceJob::Handover {
            id: space,
            handoff: directory.join("handoff.md"),
            from: old,
        };
        if let Some(worker) = &self.sessions {
            worker.send(Job::Space(job));
        }
    }

    /// Space `at`'s orchestrator's id, the space's id and its directory, when its orchestrator
    /// may be replaced now: the space open, none of its starts under way, the old one neither
    /// working nor waiting for input.
    fn replaceable(&self, at: usize) -> Result<(SessionId, SpaceId, PathBuf), String> {
        let stored = self.watch.spaces().stored(at).ok_or("No such space")?;
        let old =
            (stored.orchestrator.clone()).ok_or("The space has no orchestrator: o starts one")?;
        let directory = (stored.directory.clone()).ok_or("The space has no directory")?;
        if stored.archived {
            return Err("The space is closed".to_owned());
        }
        if let Some(why) = self.space_starts_busy(at) {
            return Err(why);
        }
        let old_session = (self.hosting.table().iter()).find(|session| session.id == old);
        let state = old_session.and_then(|session| hosted_state(session, &self.watch));
        if let Some(state) = state.filter(|s| matches!(s, State::Working | State::NeedsInput(_))) {
            return Err(format!(
                "The orchestrator is {state}: wait, or stop it first"
            ));
        }
        Ok((old, stored.id.clone(), directory))
    }

    /// What the probe found for space `id`'s replacement of session `from`: with no
    /// `handoff.md`, the person is asked first (Q2). Nothing is done when `from` is no longer
    /// the space's orchestrator.
    pub(in crate::app) fn handover_looked(
        &mut self,
        id: &SpaceId,
        from: &SessionId,
        (handoff, transcript): (bool, Option<PathBuf>),
    ) {
        self.hosting.spaces.probing.remove(id);
        let Some(at) = self.watch.spaces().find(id) else {
            return;
        };
        let recorded = (self.watch.spaces().stored(at)).and_then(|s| s.orchestrator.clone());
        if recorded.as_ref() != Some(from) {
            return (self.editor)
                .message("Not replaced: the space's orchestrator changed meanwhile".to_owned());
        }
        if handoff {
            return self.hand_over(id, transcript);
        }
        let name = self.space_label(at);
        let question = format!(
            "No handoff.md in space {name}'s directory; replace its orchestrator anyway? (y or n)"
        );
        match self.ask_confirm(question) {
            Ok(prompt) => {
                self.hosting.spaces.replace_asked = Some(Asked {
                    prompt,
                    space: id.clone(),
                    transcript,
                });
            }
            Err(e) => self.editor.message(e.to_string()),
        }
    }

    /// `y` to the question of a replacement: done, if it still may be.
    pub(in crate::app) fn replace_confirmed(&mut self) {
        if let Some(asked) = self.hosting.spaces.replace_asked.take() {
            self.hand_over(&asked.space, asked.transcript);
        }
    }

    /// Replaces space `id`'s orchestrator: the old one renamed, kept as an earlier one and
    /// stopped, the successor started once it ended, or at once if it does not run.
    fn hand_over(&mut self, id: &SpaceId, transcript: Option<PathBuf>) {
        let Some(at) = self.watch.spaces().find(id) else {
            return;
        };
        // Looked at again: the old one may have got busy meanwhile.
        let old = match self.replaceable(at) {
            Ok((old, ..)) => old,
            Err(why) => return self.editor.message(why),
        };
        let Some(space_name) = self.watch.spaces().stored(at).map(|s| s.name.clone()) else {
            return;
        };
        let table = self.hosting.table();
        let n = next_earlier(
            &space_name,
            table.iter().map(|session| session.name.as_str()),
        );
        let renamed = earlier_name(&space_name, n);
        let sessions: Vec<(String, String, PathBuf)> = (table.iter())
            .filter(|session| session.id != old && self.space_of_session(session) == Some(at))
            .map(|session| {
                let state = hosted_state(session, &self.watch)
                    .map_or_else(|| session.run.label().to_owned(), |state| state.to_string());
                (session.name.clone(), state, session.directory.clone())
            })
            .collect();
        let brief = takeover_brief(
            &space_name,
            (&renamed, old.as_str()),
            transcript.as_deref(),
            &sessions,
        );
        let key = (table.iter())
            .find(|session| session.id == old)
            .map(|session| (session.key, session.runs() && session.buffer.is_some()));
        if let Some((key, _)) = key
            && let Some(session) = self.hosting.table.get_mut(key)
        {
            session.name.clone_from(&renamed);
            self.hosting.unsaved = true;
        }
        // From now on the old one is an earlier orchestrator: a resume of it reaches nothing.
        self.watch.spaces_mut().retire_orchestrator(at, now_ms());
        let shown = super::printable(&renamed);
        match key {
            Some((key, true)) => {
                self.editor.message(format!(
                    "Stopping the orchestrator, kept as {shown}; its successor starts once it ended"
                ));
                let successor = Successor {
                    space: id.clone(),
                    brief,
                };
                self.hosting.spaces.replacing.insert(key, successor);
                self.hang_up_session(key);
            }
            _ => {
                (self.editor).message(format!(
                    "The orchestrator is kept as {shown}; its successor starts"
                ));
                self.launch_orchestrator(at, Some(brief));
            }
        }
        self.sessions_touched();
    }

    /// Hosted session `key` ended, its program `exited` or not: when it was an orchestrator
    /// being replaced, its successor starts now, so that never two run under the space's name
    /// (Q1). A program that outlived its terminal may still run: then the person starts the
    /// successor with `o` once it ended.
    pub(in crate::app) fn replaced_orchestrator_ended(&mut self, key: SessionKey, exited: bool) {
        let Some(successor) = self.hosting.spaces.replacing.remove(&key) else {
            return;
        };
        if !exited {
            return self.editor.message(
                "The old orchestrator outlived its terminal: o starts its successor once it ended"
                    .to_owned(),
            );
        }
        if let Some(at) = self.watch.spaces().find(&successor.space) {
            self.launch_orchestrator(at, Some(successor.brief));
        }
    }

    /// Why space `at` takes no other start of an orchestrator now: one is starting, its
    /// replacement waits for the old one to end, or its replacement's probe is out.
    pub(in crate::app) fn space_starts_busy(&self, at: usize) -> Option<String> {
        let id = &self.watch.spaces().stored(at)?.id;
        let spaces = &self.hosting.spaces;
        let table = self.hosting.table();
        let ending = (spaces.replacing.iter())
            .any(|(key, next)| next.space == *id && table.get(*key).is_some_and(|s| s.runs()));
        if ending || spaces.probing.contains(id) {
            return Some("Its orchestrator is being replaced".to_owned());
        }
        (spaces.starting.values().any(|space| space == id))
            .then(|| "Its orchestrator is starting".to_owned())
    }

    /// Space `at` closed: no successor starts in it, nor an orchestrator once its directory is
    /// made.
    pub(in crate::app) fn forget_space_starts(&mut self, at: usize) {
        let Some(id) = self.watch.spaces().stored(at).map(|s| s.id.clone()) else {
            return;
        };
        let spaces = &mut self.hosting.spaces;
        spaces.placeholders.remove(&id);
        // A space closed while `claude` names it: the run is stopped, and its answer named nothing.
        if spaces.asked_names.remove(&id).is_some()
            && let Some(running) = spaces.summary_run.take()
        {
            crate::sessions::spaces::summary::stop(&running);
        }
        spaces.replacing.retain(|_, next| next.space != id);
        spaces.start_after.remove(&id);
        spaces.probing.remove(&id);
    }
}
