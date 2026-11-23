//! A space's orchestrator in the app (ADR 0136, step 3): starting it in the space's own
//! directory, its directory made first, or made again where it is gone; recording it; making
//! its command line again for a resume. Its offers of a repository added are `offers`'.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::Path;

use crate::app::App;
use crate::app::sessions::hosting::{Begin, Then};
use crate::sessions::spaces::orchestrator::{Repository, TEXT, header};
use crate::sessions::spaces::stored::{now_ms, same};
use crate::sessions::spaces::{Member, SpaceId};
use crate::sessions::start::SpaceStart;
use crate::sessions::worker::{Job, SessionKey, SpaceJob};

/// The orchestrator of the space in front, as a review that talks to it sees it (ADR 0163).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum Orchestrator {
    /// Running.
    Running(SessionKey),
    /// Stopped, with its terminal, or a native one's transcript (ADR 0176), where
    /// `session-resume` takes it up again.
    Stopped { name: String, key: SessionKey },
    /// Recorded, with no terminal.
    NoTerminal { name: String },
}

impl App {
    /// `space-orchestrator-start`: the space in front's orchestrator, shown if it runs; else
    /// started in the space's directory, which is made first if it has none, and made again
    /// where it is gone with `remake`.
    pub(in crate::app) fn start_orchestrator(&mut self, remake: bool) {
        let Some(at) = self.watch.spaces().current() else {
            return self.editor.message("No space in front".to_owned());
        };
        if let Some(key) = self.live_orchestrator(at) {
            self.editor
                .message("The space's orchestrator runs: here it is".to_owned());
            return self.show_session(key);
        }
        if let Some(why) = self.space_starts_busy(at) {
            return self.editor.message(why);
        }
        let Some(stored) = self.watch.spaces().stored(at) else {
            return;
        };
        let id = stored.id.clone();
        match stored.directory.clone() {
            None => self.make_space_directory(at, false, true),
            Some(directory) if remake => self.remake_space_directory(at, &id, &directory),
            Some(_) => self.launch_orchestrator(at, None),
        }
    }

    /// Makes space `at`'s directory again at its recorded place, `directory`, which must be
    /// gone: Claude Code keys the orchestrator's memory by that path.
    fn remake_space_directory(&mut self, at: usize, id: &SpaceId, directory: &Path) {
        let (Some(root), Some(name)) = (directory.parent(), directory.file_name()) else {
            return;
        };
        let Some(stored) = self.watch.spaces().stored(at) else {
            return;
        };
        let job = SpaceJob::MakeDirectory {
            id: id.clone(),
            root: root.to_path_buf(),
            name: name.to_string_lossy().into_owned(),
            members: stored.members.clone(),
            taken: Vec::new(),
            exact: true,
        };
        if self.hosting.spaces.making.contains(id) {
            return self
                .editor
                .message("The space's directory is being made".to_owned());
        }
        if let Some(worker) = &self.sessions {
            worker.send(Job::Space(job));
            self.hosting.spaces.start_after.insert(id.clone());
            self.hosting.spaces.making.insert(id.clone());
        }
    }

    /// Starts space `at`'s orchestrator in its directory, named after the space. Whole, once
    /// the configuration's first look is done; meanwhile it counts as starting (ADR 0173).
    pub(in crate::app) fn launch_orchestrator(&mut self, at: usize, brief: Option<String>) {
        if self.starts_wait() {
            let Some(id) = self.watch.spaces().stored(at).map(|s| s.id.clone()) else {
                return;
            };
            self.hosting.spaces.waiting.insert(id.clone());
            return self.hold_start(move |app| app.launch_held_orchestrator(&id, brief));
        }
        let space = match self.orchestrator_start(at) {
            Ok(space) => space,
            Err(why) => return self.editor.message(why),
        };
        let Some(stored) = self.watch.spaces().stored(at) else {
            return;
        };
        let (id, name, directory) = (
            stored.id.clone(),
            stored.name.clone(),
            stored.directory.clone(),
        );
        // Shown in its space: one whose start comes after the person left it starts behind, as
        // a successor's may (ADR 0159), and so does `space-new`'s once the person put something
        // else where its `*scratch*` was, as it would take the keys they type there.
        let placeholder = self.hosting.spaces.placeholders.remove(&id);
        let behind = self.watch.spaces().current() != Some(at)
            || placeholder.is_some_and(|scratch| self.editor.window().buffer() != scratch);
        let begin = Begin {
            name: Some(name),
            directory,
            brief,
            space: Some(space),
            behind,
            ..Begin::default()
        };
        self.begin_session(begin, Then::Orchestrator(id));
    }

    /// Starts space `id`'s orchestrator, which waited: nothing if the space closed meanwhile.
    fn launch_held_orchestrator(&mut self, id: &SpaceId, brief: Option<String>) {
        if !self.hosting.spaces.waiting.remove(id) {
            return;
        }
        if let Some(at) = self.watch.spaces().find(id) {
            self.launch_orchestrator(at, brief);
        }
    }

    /// Hosted session `key` just started: as an orchestrator, it belongs to its space, which
    /// records it, the one before kept as an earlier one; started by one, it is a worker of
    /// that space (ADR 0136, step 4).
    pub(in crate::app) fn space_session_started(&mut self, key: SessionKey) {
        // A session that moved into a space at its request is in that space, if still open
        // (ADR 0151).
        if let Some(space) = self.moving_into(key).cloned() {
            let spaces = self.watch.spaces();
            let open =
                (spaces.find(&space).and_then(|at| spaces.get(at))).is_some_and(|s| s.is_open());
            if let Some(session) = self.hosting.table.get_mut(key)
                && open
            {
                session.space = Some(space);
                self.hosting.unsaved = true;
                self.sessions_touched();
            }
            return;
        }
        let Some(space) = self.hosting.spaces.starting.remove(&key) else {
            return self.worker_joined(key);
        };
        let Some(session) = self.hosting.table.get_mut(key) else {
            return;
        };
        session.space = Some(space.clone());
        let session_id = session.id.clone();
        self.hosting.unsaved = true;
        let spaces = self.watch.spaces_mut();
        if let Some(at) = spaces.find(&space) {
            spaces.set_orchestrator(at, session_id, now_ms());
        }
        self.sessions_touched();
    }

    /// Session `key`, forgotten as it ended before it was given a message: if it was a space's
    /// orchestrator, the one before it is again, as nothing is left of it to resume
    /// (ADR 0176, the author's Q-O8 (a)).
    pub(in crate::app) fn unspoken_orchestrator_forgotten(&mut self, key: SessionKey) {
        let Some(id) = self
            .hosting
            .table
            .get(key)
            .map(|session| session.id.clone())
        else {
            return;
        };
        let spaces = self.watch.spaces_mut();
        let Some(at) = spaces.orchestrated_by(&id) else {
            return;
        };
        spaces.forget_orchestrator(at, &id, now_ms());
        self.sessions_touched();
    }

    /// What hosted session `key` starts with again, when it is a space's orchestrator: its
    /// space as it is now, as Claude Code keeps neither its `--add-dir`s nor its prompt. Another
    /// session in a space's directory gets the directory's checks alone.
    ///
    /// # Errors
    ///
    /// Why the orchestrator cannot start, for the echo area.
    pub(in crate::app) fn orchestrator_start_for(
        &mut self,
        key: SessionKey,
    ) -> Result<Option<SpaceStart>, String> {
        let Some(session) = self.hosting.table().get(key) else {
            // A new orchestrator whose start was held: its space as it is now.
            let starting = self.hosting.spaces.starting.get(&key);
            let at = starting.and_then(|space| self.watch.spaces().find(space));
            return at.map(|at| self.orchestrator_start(at)).transpose();
        };
        let (id, directory) = (session.id.clone(), session.directory.clone());
        let spaces = self.watch.spaces();
        let at = spaces
            .iter()
            .position(|space| space.stored.orchestrator.as_ref() == Some(&id));
        match at {
            Some(at) => self.orchestrator_start(at).map(Some),
            None => Ok(self
                .space_checks(&directory)
                .or_else(|| self.reads_again(key))),
        }
    }

    /// The checks a session starting in `directory` passes when it is a space's directory,
    /// with no `--add-dir` and no prompt (ADR 0136: before every start there).
    pub(in crate::app) fn space_checks(&self, directory: &Path) -> Option<SpaceStart> {
        let stored = self
            .watch
            .spaces()
            .stored(self.space_at_directory(directory)?)?;
        Some(SpaceStart {
            in_space_directory: true,
            reads: Vec::new(),
            made: stored.directory_identity,
            members: stored.members.clone(),
            trusted: Vec::new(),
            prompt: String::new(),
            instructions: None,
        })
    }

    /// What space `at`'s orchestrator starts with, the repositories it is given kept so that
    /// none is offered to it again.
    fn orchestrator_start(&mut self, at: usize) -> Result<SpaceStart, String> {
        let start = self.orchestrator_start_of(at)?;
        if let Some(stored) = self.watch.spaces().stored(at) {
            let given = start.trusted.iter().map(|member| member.root.clone());
            (self.hosting.spaces.given).insert(stored.id.clone(), given.collect());
        }
        Ok(start)
    }

    /// What space `at`'s orchestrator starts with: the fixed text, the space's header, its
    /// repositories trusted by path and identity, as oso trusts a project, and its
    /// instructions' file.
    fn orchestrator_start_of(&self, at: usize) -> Result<SpaceStart, String> {
        let stored = self.watch.spaces().stored(at).ok_or("No such space")?;
        if stored.archived {
            return Err("The space is closed: space-reopen opens it again".to_owned());
        }
        if stored.directory.is_none() {
            return Err("The space has no directory yet: space-directory makes it".to_owned());
        }
        let named = self.watch.spaces().member_names(at);
        let trusted = |root: &Path| {
            (stored.members.iter()).any(|member| same(&member.root, root) && self.trusts(member))
        };
        let repositories: Vec<Repository<'_>> = named
            .iter()
            .map(|(name, root)| Repository {
                name,
                root,
                trusted: trusted(root),
            })
            .collect();
        let trusted_members: Vec<Member> = (stored.members.iter())
            .filter(|member| self.trusts(member))
            .cloned()
            .collect();
        let prompt = format!("{TEXT}{}", header(stored, &repositories));
        let instructions = self.hosting.state_directory().map(|state| {
            crate::sessions::spaces::probe::instructions_path(&state, stored.id.as_str())
        });
        Ok(SpaceStart {
            in_space_directory: true,
            reads: Vec::new(),
            made: stored.directory_identity,
            members: stored.members.clone(),
            trusted: trusted_members,
            prompt,
            instructions,
        })
    }

    /// The orchestrator of the space in front, while one is recorded and one of `places` is
    /// in that space: a member repository, a worktree the space made or holds, or a worktree
    /// git lists under a member. `None` otherwise, so that a review of another repository
    /// talks to its own session (ADR 0163).
    pub(in crate::app) fn space_orchestrator_of(&self, places: &[&Path]) -> Option<Orchestrator> {
        let spaces = self.watch.spaces();
        let at = spaces.current()?;
        let stored = spaces.stored(at)?;
        stored.orchestrator.as_ref()?;
        let same = oso_core::project::same_place;
        let member = |root: &Path| stored.members.iter().any(|m| same(&m.root, root));
        let inside = places.iter().any(|place| {
            member(place)
                || stored.worktrees.iter().any(|w| same(w, place))
                || self
                    .listed_repository_of(place)
                    .is_some_and(|repo| member(&repo))
        });
        if !inside {
            return None;
        }
        if let Some(key) = self.live_orchestrator(at) {
            return Some(Orchestrator::Running(key));
        }
        let name = stored.name.clone();
        Some(match self.orchestrator_of(at) {
            Some(key) => Orchestrator::Stopped { name, key },
            None => Orchestrator::NoTerminal { name },
        })
    }

    /// The key of space `at`'s orchestrator while it runs.
    pub(in crate::app) fn live_orchestrator(&self, at: usize) -> Option<SessionKey> {
        let id = self.watch.spaces().stored(at)?.orchestrator.clone()?;
        let table = self.hosting.table();
        table
            .iter()
            .find(|session| session.id == id && session.runs())
            .map(|session| session.key)
    }

    /// The key of space `at`'s orchestrator, running or stopped, while it has a terminal, or a
    /// native one's transcript.
    pub(in crate::app) fn orchestrator_of(&self, at: usize) -> Option<SessionKey> {
        let id = self.watch.spaces().stored(at)?.orchestrator.clone()?;
        let table = self.hosting.table();
        table
            .iter()
            .find(|session| session.id == id && !session.archived && session.buffer.is_some())
            .map(|session| session.key)
    }
}
