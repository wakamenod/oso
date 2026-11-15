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
use crate::sessions::spaces::SpaceId;
use crate::sessions::spaces::orchestrator::{earlier_name, takeover_brief};
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
    /// may be replaced now.
    fn replaceable(
        &self,
        at: usize,
    ) -> Result<(crate::sessions::identity::SessionId, SpaceId, PathBuf), String> {
        let stored = self.watch.spaces().stored(at).ok_or("No such space")?;
        let old =
            (stored.orchestrator.clone()).ok_or("The space has no orchestrator: o starts one")?;
        let directory = stored
            .directory
            .clone()
            .ok_or("The space has no directory")?;
        if (self.hosting.spaces.replacing.values()).any(|next| next.space == stored.id) {
            return Err("Its orchestrator is being replaced".to_owned());
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

    /// What the probe found for space `id`'s replacement: with no `handoff.md`, the person is
    /// asked first (Q2).
    pub(in crate::app) fn handover_looked(
        &mut self,
        id: &SpaceId,
        handoff: bool,
        transcript: Option<PathBuf>,
    ) {
        if handoff {
            return self.hand_over(id, transcript);
        }
        let Some(at) = self.watch.spaces().find(id) else {
            return;
        };
        let name = self.space_name(at);
        let question = format!(
            "No handoff.md in space {name}'s directory; replace its orchestrator anyway? (y or n)"
        );
        self.hosting.spaces.offer = None;
        match self.editor.ask(question, "space-confirm") {
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

    /// Replaces space `id`'s orchestrator: the old one renamed and stopped, the successor
    /// started once it ended, or at once if it does not run.
    fn hand_over(&mut self, id: &SpaceId, transcript: Option<PathBuf>) {
        let Some(at) = self.watch.spaces().find(id) else {
            return;
        };
        // Looked at again: the old one may have got busy meanwhile.
        let old = match self.replaceable(at) {
            Ok((old, ..)) => old,
            Err(why) => return self.editor.message(why),
        };
        let Some(stored) = self.watch.spaces().stored(at) else {
            return;
        };
        let (space_name, earlier) = (stored.name.clone(), stored.earlier.len());
        let renamed = earlier_name(&space_name, earlier);
        let sessions: Vec<(String, String, PathBuf)> = (self.hosting.table().iter())
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
        let key = (self.hosting.table().iter())
            .find(|session| session.id == old)
            .map(|session| (session.key, session.runs()));
        if let Some((key, _)) = key
            && let Some(session) = self.hosting.table.get_mut(key)
        {
            session.name.clone_from(&renamed);
            self.hosting.unsaved = true;
        }
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
                self.editor.message(format!(
                    "The orchestrator is kept as {shown}; its successor starts"
                ));
                self.launch_orchestrator(at, Some(brief));
            }
        }
        self.sessions_touched();
    }

    /// Hosted session `key` ended: when it was an orchestrator being replaced, its successor
    /// starts now, so that never two run under the space's name (Q1).
    pub(in crate::app) fn replaced_orchestrator_ended(&mut self, key: SessionKey) {
        let Some(successor) = self.hosting.spaces.replacing.remove(&key) else {
            return;
        };
        if let Some(at) = self.watch.spaces().find(&successor.space) {
            self.launch_orchestrator(at, Some(successor.brief));
        }
    }
}
