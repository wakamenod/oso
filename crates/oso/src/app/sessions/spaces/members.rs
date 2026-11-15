//! A space's repositories as oso trusts them, the sessions that join a space as its workers,
//! the worktrees a space holds when they are made, and the repository the project commands
//! use from an orchestrator's buffer (ADR 0136, step 4).
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk.

use std::path::{Path, PathBuf};

use oso_core::buffer::BufferId;

use crate::app::App;
use crate::git::TrustedRoot;
use crate::sessions::identity::SessionId;
use crate::sessions::spaces::Member;
use crate::sessions::spaces::stored::now_ms;
use crate::sessions::start::SpaceStart;
use crate::sessions::tools::repo;
use crate::sessions::worker::SessionKey;
use crate::sessions::worktree::jobs::Trust;

impl App {
    /// The trust of space repository `member`, if oso trusts it by its path and the identity
    /// the space kept, so that one made again at a trusted path is not, nor one with no
    /// identity kept where the system has them. Made fresh for each use, and never revoked:
    /// each use asks the trust list again.
    pub(in crate::app) fn member_trust(&self, member: &Member) -> Option<TrustedRoot> {
        let list = self.editor.trust_list()?;
        TrustedRoot::of_member(list, &member.root, member.identity)
    }

    /// Whether oso trusts space repository `member` now.
    pub(in crate::app) fn trusts(&self, member: &Member) -> bool {
        self.member_trust(member).is_some()
    }

    /// What a worker that also reads `reads` starts with: those directories, each with its
    /// repository's trust, checked again with git on its start thread (ADR 0136, step 4b);
    /// nothing for none.
    pub(in crate::app) fn worker_reads(reads: Vec<(Trust, PathBuf)>) -> Option<SpaceStart> {
        (!reads.is_empty()).then(|| SpaceStart {
            in_space_directory: false,
            made: None,
            members: Vec::new(),
            trusted: Vec::new(),
            reads,
            prompt: String::new(),
            instructions: None,
        })
    }

    /// The directories a start with `space` also reads: a worker's; none for another.
    pub(in crate::app) fn reads_of(space: Option<&SpaceStart>) -> Vec<PathBuf> {
        let reads = space.map_or(&[][..], |space| space.reads.as_slice());
        reads.iter().map(|(_, dir)| dir.clone()).collect()
    }

    /// `reads` as they stand at the person's `y`, or before a start: those whose repository oso
    /// still trusts, with the trusted roots as they are now; the others are said and left out.
    pub(in crate::app) fn reads_now(
        &mut self,
        reads: Vec<(Trust, PathBuf)>,
    ) -> Vec<(Trust, PathBuf)> {
        let (kept, left): (Vec<_>, Vec<_>) = reads
            .into_iter()
            .partition(|(trust, _)| self.still_trusted(&trust.root));
        self.say_left_out(left.iter().map(|(_, dir)| dir.as_path()));
        let trusted = self.trusted_list();
        (kept.into_iter())
            .map(|(trust, dir)| {
                let trusted = trusted.clone();
                (Trust { trusted, ..trust }, dir)
            })
            .collect()
    }

    /// `space` with its worker's directories as they stand now ([`App::reads_now`]).
    pub(in crate::app) fn space_now(&mut self, space: Option<SpaceStart>) -> Option<SpaceStart> {
        match space {
            Some(space) if !space.in_space_directory => {
                Self::worker_reads(self.reads_now(space.reads))
            }
            other => other,
        }
    }

    /// The directories session `key` also reads, for its resume (Q1), when it is a worker of
    /// an open space, started by that space's orchestrator, current or earlier: each a trusted
    /// repository's root or a worktree the space holds, with that repository's trust, for its start thread to
    /// check with git as the tool's job did. The record is one agents can write, so nothing in
    /// it is taken as checked. What is given, and what is left out, is said.
    pub(in crate::app) fn reads_again(&mut self, key: SessionKey) -> Option<SpaceStart> {
        let session = self.hosting.table.get(key)?;
        if session.reads.is_empty() {
            return None;
        }
        let spaces = self.watch.spaces();
        let stored = (self.space_of_session(session))
            .and_then(|at| spaces.stored(at))
            .filter(|stored| !stored.archived)
            .filter(|stored| {
                (session.parent.as_ref()).is_some_and(|parent| {
                    stored.orchestrator.as_ref() == Some(parent) || stored.earlier.contains(parent)
                })
            });
        let trust_of = |dir: &Path| {
            let stored = stored?;
            let member = stored.member(stored.read_root(dir)?)?;
            let root = self.member_trust(member)?;
            Some(Trust {
                root,
                trusted: self.trusted_list(),
            })
        };
        let mut given = Vec::new();
        let mut left = Vec::new();
        for read in &session.reads {
            match trust_of(&read.root) {
                Some(trust) => given.push((trust, read.root.clone())),
                None => left.push(read.root.clone()),
            }
        }
        self.say_left_out(left.iter().map(PathBuf::as_path));
        if !given.is_empty() {
            let shown: Vec<String> = (given.iter())
                .map(|(_, dir)| super::printable(&dir.display().to_string()))
                .collect();
            (self.editor).message(format!(
                "Given again to read, once checked: {}",
                shown.join(", ")
            ));
        }
        Self::worker_reads(given)
    }

    /// Says which directories to read are left out.
    fn say_left_out<'a>(&mut self, left: impl Iterator<Item = &'a Path>) {
        let shown: Vec<String> = left
            .map(|dir| super::printable(&dir.display().to_string()))
            .collect();
        if !shown.is_empty() {
            (self.editor).message(format!(
                "Not given to read, as no trusted repository of the space holds it now: {}",
                shown.join(", ")
            ));
        }
    }

    /// Whether `root` is still trusted, by path and identity, and not taken back.
    pub(in crate::app) fn still_trusted(&self, root: &TrustedRoot) -> bool {
        !root.revoked()
            && (self.editor.trust_list()).is_some_and(|list| {
                TrustedRoot::of_member(list, root.path(), root.identity()).is_some()
            })
    }

    /// Hosted session `key`, about to start as `space` says: the directories it also reads
    /// are kept for its record, which a resume checks again before it passes them.
    pub(in crate::app) fn note_reads(&mut self, key: SessionKey, space: Option<&SpaceStart>) {
        let dirs = Self::reads_of(space);
        if !dirs.is_empty() {
            let reads = dirs.iter().map(|dir| Member::at(dir)).collect();
            (self.hosting.spaces.reads).insert(key, reads);
        }
    }

    /// Hosted session `key`, just started: when its parent is a space's orchestrator, it is a
    /// worker of that space; the directories it also reads go in its record.
    pub(super) fn worker_joined(&mut self, key: SessionKey) {
        if let Some(reads) = self.hosting.spaces.reads.remove(&key)
            && let Some(session) = self.hosting.table.get_mut(key)
        {
            session.reads = reads;
            self.hosting.unsaved = true;
        }
        let Some(parent) = self.hosting.table.get(key).and_then(|s| s.parent.clone()) else {
            return;
        };
        let spaces = self.watch.spaces();
        let Some(id) = (spaces.orchestrated_by(&parent))
            .and_then(|at| spaces.stored(at))
            .map(|stored| stored.id.clone())
        else {
            return;
        };
        if let Some(session) = self.hosting.table.get_mut(key) {
            session.space = Some(id);
            self.hosting.unsaved = true;
            self.sessions_touched();
        }
    }

    /// Holds the worktree at `path` of repository `main`, just made for the session `asker`:
    /// in the asker's space when `main` is one of its repositories, else in the space it joins.
    pub(in crate::app) fn hold_made_worktree(
        &mut self,
        asker: Option<&SessionId>,
        path: &Path,
        main: &Path,
    ) {
        let asker = asker
            .and_then(|id| self.hosting.table.iter().find(|session| session.id == *id))
            .and_then(|session| self.space_of_session(session));
        (self.watch.spaces_mut()).hold_made_worktree(asker, path, main, now_ms());
    }

    /// The name of the space session `id` orchestrates, for its workers' briefs.
    pub(in crate::app) fn orchestrated_space(&self, id: &SessionId) -> Option<String> {
        let spaces = self.watch.spaces();
        let at = spaces.orchestrated_by(id)?;
        spaces.get(at).map(|space| space.name().to_owned())
    }

    /// `space-use-repo`: the space in front's trusted repository `name` for the project
    /// commands run from its orchestrator's buffer.
    pub(in crate::app) fn use_repository(&mut self, name: &str) {
        let Some(at) = self.watch.spaces().current() else {
            return self.editor.message("No space in front".to_owned());
        };
        let shown = super::printable(name);
        let spaces = self.watch.spaces();
        let Some(stored) = spaces.stored(at) else {
            return;
        };
        let Some(member) = (spaces.member_named(name)).and_then(|root| stored.member(&root)) else {
            return (self.editor).message(format!("The space has no repository {shown}"));
        };
        if !self.trusts(member) {
            return (self.editor).message(format!(
                "{shown} is not trusted in oso: trust-project first"
            ));
        }
        let (id, root) = (stored.id.clone(), member.root.clone());
        self.hosting.spaces.used.insert(id, root);
        (self.editor).message(format!("The orchestrator's commands use {shown}"));
    }

    /// The repository the project commands use from `buffer` when it is a space's
    /// orchestrator's ([`repo::for_commands`]); `None` for another buffer. A buffer of that
    /// repository must be open for the command to find it as a project.
    pub(in crate::app) fn orchestrator_repository(
        &self,
        buffer: BufferId,
    ) -> Option<Result<PathBuf, String>> {
        let session = self.hosting.table.by_buffer(buffer)?;
        let spaces = self.watch.spaces();
        let stored = spaces.stored(spaces.orchestrated_by(&session.id)?)?;
        let trusted: Vec<&Path> = (stored.members.iter())
            .filter(|member| self.trusts(member))
            .map(|member| member.root.as_path())
            .collect();
        let chosen = self.hosting.spaces.used.get(&stored.id);
        Some(repo::for_commands(&trusted, chosen.map(PathBuf::as_path)))
    }
}
