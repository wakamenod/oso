//! A space's repositories as oso trusts them, the sessions that join a space as its workers,
//! the worktrees a space holds when they are made, and the repository the project commands
//! use from an orchestrator's buffer (ADR 0136, step 4).
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk.

use std::path::{Path, PathBuf};

use oso_core::buffer::BufferId;

use crate::app::App;
use crate::review::TrustedRoot;
use crate::sessions::identity::SessionId;
use crate::sessions::spaces::Member;
use crate::sessions::spaces::stored::now_ms;
use crate::sessions::tools::repo;
use crate::sessions::worker::SessionKey;

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

    /// Hosted session `key`, just started: when its parent is a space's orchestrator, it is a
    /// worker of that space.
    pub(super) fn worker_joined(&mut self, key: SessionKey) {
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
