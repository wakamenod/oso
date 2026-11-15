//! The session tools in an issue space (ADR 0136, step 4): a space's orchestrator acts only on
//! its space's sessions and on its repositories, named by `repo`, each by its trust as oso has
//! it now, by path and identity; any other caller keeps its repository's scope.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here touches the disk.

use std::path::Path;

use super::super::super::App;
use super::Caller;
use crate::review::TrustedRoot;
use crate::sessions::hosted::Hosted;
use crate::sessions::tools::repo::{self, Named};

impl App {
    /// The caller hosted session `session` is, with its space.
    pub(super) fn caller_of(&self, session: &Hosted) -> Caller {
        let mut caller = Caller::of(session);
        let spaces = self.watch.spaces();
        caller.space = self
            .space_of_session(session)
            .and_then(|at| spaces.stored(at))
            .map(|stored| {
                let orchestrator =
                    !stored.archived && stored.orchestrator.as_ref() == Some(&session.id);
                (stored.id.clone(), orchestrator)
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
        let own = caller.root.as_deref();
        match repo::named(caller.orchestrator(), repo, &members, own)? {
            Named::Own => Ok(caller),
            Named::Member(root) => self.at_member(caller, root),
        }
    }

    /// An orchestrator's call on `path` acts in the space's repository that holds it.
    ///
    /// # Errors
    ///
    /// Why the call is refused, for the agent.
    pub(super) fn scoped_to_path(&self, caller: Caller, path: &Path) -> Result<Caller, String> {
        if !caller.orchestrator() {
            return Ok(caller);
        }
        let members = self.caller_members(&caller);
        let root = repo::holding(&members, path)
            .ok_or("that path is in no repository of your space")?
            .to_path_buf();
        self.at_member(caller, &root)
    }

    /// `caller`, an orchestrator, acting at its space's repository `root`, which oso must
    /// trust now by path and identity.
    fn at_member(&self, mut caller: Caller, root: &Path) -> Result<Caller, String> {
        let identity = self.space_member_identity(&caller, root);
        let trusted = (self.editor.trust_list())
            .and_then(|list| TrustedRoot::of_member(list, root, identity))
            .ok_or_else(|| {
                format!(
                    "{} is not trusted in oso; the person trusts it with trust-project",
                    rules_quoted(root)
                )
            })?;
        caller.directory = root.to_path_buf();
        caller.root = Some(root.to_path_buf());
        caller.member = Some(trusted);
        Ok(caller)
    }

    /// Whether the space repository `member` is still trusted, by path and identity.
    pub(super) fn member_trusted_now(&self, member: &TrustedRoot) -> bool {
        !member.revoked()
            && (self.editor.trust_list()).is_some_and(|list| {
                TrustedRoot::of_member(list, member.path(), member.identity()).is_some()
            })
    }

    /// The repositories of `caller`'s space by name and root; none outside a space.
    fn caller_members(&self, caller: &Caller) -> Vec<(String, std::path::PathBuf)> {
        let spaces = self.watch.spaces();
        let at = (caller.space.as_ref()).and_then(|(id, _)| spaces.find(id));
        at.map(|at| spaces.member_names(at)).unwrap_or_default()
    }

    /// The identity the caller's space recorded for its repository `root`.
    fn space_member_identity(&self, caller: &Caller, root: &Path) -> Option<(u64, u64)> {
        let spaces = self.watch.spaces();
        let at = spaces.find(&caller.space.as_ref()?.0)?;
        let stored = spaces.stored(at)?;
        let member = stored.members.iter().find(|member| member.root == root)?;
        member.identity
    }

    /// The name of the space `caller` orchestrates, for its workers' briefs.
    pub(super) fn orchestrated_space(&self, caller: &Caller) -> Option<String> {
        let (id, true) = caller.space.as_ref()? else {
            return None;
        };
        let spaces = self.watch.spaces();
        let at = spaces.find(id)?;
        spaces.get(at).map(|space| space.name().to_owned())
    }

    /// Whether `caller` may see `session`: an orchestrator only its space's sessions (Q7),
    /// any other caller every one.
    pub(super) fn caller_sees(&self, caller: Option<&Caller>, session: &Hosted) -> bool {
        let Some((space, true)) = caller.and_then(|caller| caller.space.as_ref()) else {
            return true;
        };
        let spaces = self.watch.spaces();
        self.space_of_session(session)
            .and_then(|at| spaces.stored(at))
            .is_some_and(|stored| &stored.id == space)
    }
}

/// A path as the agent is told it.
fn rules_quoted(path: &Path) -> String {
    crate::sessions::tools::quoted(&path.display().to_string())
}
