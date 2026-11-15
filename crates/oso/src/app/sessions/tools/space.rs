//! The session tools in an issue space (ADR 0136, step 4): a space's orchestrator acts only on
//! its space's sessions and on its repositories, named by `repo`, each by its trust as oso has
//! it now, by path and identity; any other caller keeps its repository's scope.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk.
//!
//! - The orchestrator is the session its space records, while it runs in the space's own
//!   directory: a record agents can write names no other session one. A closed space's
//!   orchestrator, an earlier one, and one elsewhere still see only the space's sessions, and
//!   reach no repository.

use std::path::{Path, PathBuf};

use super::super::super::App;
use super::Caller;
use crate::review::TrustedRoot;
use crate::sessions::hosted::Hosted;
use crate::sessions::spaces::SpaceId;
use crate::sessions::spaces::stored::same;
use crate::sessions::tools::repo::{self, Named};

/// A caller's space, and its part there.
#[derive(Clone, Debug)]
pub(in crate::app) struct InSpace {
    pub(super) id: SpaceId,
    pub(super) role: Role,
}

/// A session's part in its space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Role {
    /// The space's orchestrator, in the space's directory, the space open.
    Orchestrator,
    /// An orchestrator no more, or not here: one of a closed space, an earlier one, or one not
    /// in the space's directory. It sees the space's sessions only, and reaches no repository.
    Former,
    /// Any other session of the space, which keeps its repository's scope (Q6).
    Worker,
}

impl App {
    /// The caller hosted session `session` is, with its space and its part there.
    pub(super) fn caller_of(&self, session: &Hosted) -> Caller {
        let mut caller = Caller::of(session);
        let spaces = self.watch.spaces();
        let current = spaces.orchestrated_by(&session.id).map(|at| {
            let stored = spaces.stored(at);
            let here = stored.and_then(|s| s.directory.as_deref());
            let open = stored.is_some_and(|s| !s.archived);
            let role = if open && here.is_some_and(|dir| same(dir, &session.directory)) {
                Role::Orchestrator
            } else {
                Role::Former
            };
            (at, role)
        });
        let earlier = || {
            (0..spaces.iter().count())
                .find(|&at| (spaces.stored(at)).is_some_and(|s| s.earlier.contains(&session.id)))
                .map(|at| (at, Role::Former))
        };
        let worker = || self.space_of_session(session).map(|at| (at, Role::Worker));
        caller.space = current
            .or_else(earlier)
            .or_else(worker)
            .and_then(|(at, role)| {
                let id = spaces.stored(at)?.id.clone();
                Some(InSpace { id, role })
            });
        caller
    }

    /// `caller` as a call naming `repo` reaches: an orchestrator at the space's repository it
    /// names; another caller as it is, `repo` naming its own repository if given. A connection
    /// that is no hosted session names none.
    ///
    /// # Errors
    ///
    /// Why the call is refused, for the agent.
    pub(super) fn scoped(
        &self,
        caller: Option<Caller>,
        repo: Option<&str>,
    ) -> Result<Option<Caller>, String> {
        match (caller, repo) {
            (Some(caller), repo) => self.scoped_caller(caller, repo).map(Some),
            (None, None) => Ok(None),
            (None, Some(_)) => Err("\"repo\" is only for a session oso started".to_owned()),
        }
    }

    /// As [`App::scoped`], for a hosted session's call.
    ///
    /// # Errors
    ///
    /// Why the call is refused, for the agent.
    pub(super) fn scoped_caller(
        &self,
        caller: Caller,
        repo: Option<&str>,
    ) -> Result<Caller, String> {
        let members = self.caller_members(&caller);
        let named = if caller.orchestrator() {
            repo::for_orchestrator(repo, &members)?
        } else {
            repo::for_other(repo, &members, caller.root.as_deref())?
        };
        match named {
            Named::Own => Ok(caller),
            Named::Member(root) => self.at_member(caller, root),
        }
    }

    /// An orchestrator's call on the worktree at `path` acts in the space's repository git
    /// listed it under, else the one whose root holds it.
    ///
    /// # Errors
    ///
    /// Why the call is refused, for the agent.
    pub(super) fn scoped_to_path(&self, caller: Caller, path: &Path) -> Result<Caller, String> {
        if !caller.orchestrator() {
            return Ok(caller);
        }
        let members = self.caller_members(&caller);
        let listed = (self.listed_repository_of(path))
            .filter(|root| members.iter().any(|(_, member)| same(member, root)));
        let root = listed
            .or_else(|| repo::holding(&members, path).map(Path::to_path_buf))
            .ok_or("that path is in no repository of your space")?;
        self.at_member(caller, &root)
    }

    /// `caller`, an orchestrator, acting at its space's repository `root`, which oso must
    /// trust now by path and identity.
    fn at_member(&self, mut caller: Caller, root: &Path) -> Result<Caller, String> {
        let spaces = self.watch.spaces();
        let member = (caller.space.as_ref())
            .and_then(|space| spaces.stored(spaces.find(&space.id)?))
            .and_then(|stored| stored.member(root));
        let trusted = member
            .and_then(|member| self.member_trust(member))
            .ok_or_else(|| {
                format!(
                    "{} is not trusted in oso; the person trusts it with trust-project",
                    quoted_path(root)
                )
            })?;
        caller.directory = trusted.path().to_path_buf();
        caller.root = Some(trusted.path().to_path_buf());
        caller.acting_at = Some(trusted);
        Ok(caller)
    }

    /// The name of the space `caller` orchestrates, for its workers' briefs.
    pub(super) fn space_orchestrated_by(&self, caller: &Caller) -> Option<String> {
        caller
            .orchestrator()
            .then(|| self.orchestrated_space(&caller.id))
            .flatten()
    }

    /// Whether `root` is still trusted, by path and identity, and not taken back.
    pub(super) fn still_trusted(&self, root: &TrustedRoot) -> bool {
        !root.revoked()
            && (self.editor.trust_list()).is_some_and(|list| {
                TrustedRoot::of_member(list, root.path(), root.identity()).is_some()
            })
    }

    /// The repositories of `caller`'s space by name and root; none outside a space.
    fn caller_members(&self, caller: &Caller) -> Vec<(String, PathBuf)> {
        let spaces = self.watch.spaces();
        let at = (caller.space.as_ref()).and_then(|space| spaces.find(&space.id));
        at.map(|at| spaces.member_names(at)).unwrap_or_default()
    }

    /// Whether `caller` may see `session`: an orchestrator, current or not, only its space's
    /// sessions (Q7); any other caller every one.
    pub(super) fn caller_sees(&self, caller: Option<&Caller>, session: &Hosted) -> bool {
        match caller.and_then(|caller| caller.scoped().then_some(caller.space.as_ref()?)) {
            Some(space) => self.session_in_space(session, &space.id),
            None => true,
        }
    }

    /// The sessions in the worktree at `path`, as `worktree_list` names them, of space `id`
    /// only: any other is counted, not named (Q7).
    pub(super) fn space_sessions_in(&self, path: &Path, id: &SpaceId) -> Vec<String> {
        let mut named = self.sessions_in_where(path, |s| self.session_in_space(s, id));
        let table = self.hosting.table.all();
        let others = (table.iter())
            .filter(|s| s.directory.starts_with(path) && !self.session_in_space(s, id))
            .count();
        if others > 0 {
            named.push(format!("{others} outside your space"));
        }
        named
    }

    /// Whether a session outside `caller`'s space, for an orchestrator, is in `path`.
    pub(super) fn others_in(&self, caller: &Caller, path: &Path) -> bool {
        let Some(space) = caller.space.as_ref().filter(|_| caller.scoped()) else {
            return false;
        };
        let table = self.hosting.table.all();
        (table.iter())
            .any(|s| s.directory.starts_with(path) && !self.session_in_space(s, &space.id))
    }

    /// Whether `session` is one of space `id`'s.
    pub(super) fn session_in_space(&self, session: &Hosted, id: &SpaceId) -> bool {
        let spaces = self.watch.spaces();
        self.space_of_session(session)
            .and_then(|at| spaces.stored(at))
            .is_some_and(|stored| &stored.id == id)
    }
}

/// A path as the agent is told it.
fn quoted_path(path: &Path) -> String {
    crate::sessions::tools::quoted(&path.display().to_string())
}
