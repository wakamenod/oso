//! The trust a session's tools act under (ADR 0121, ADR 0132, ADR 0135): that of an open buffer
//! of its project, its own transcript among them (ADR 0165), else what its session started
//! under.
//!
//! Must not depend on: the disk; the editor's projects and what the app keeps say it.

use super::Caller;
use crate::app::App;
use crate::git::TrustedRoot;
use crate::sessions::worktree::jobs::Trust;

/// The trust a session's tools act under: as an open buffer of its project has it, trusted or
/// not, else what is kept of its start. Where both are of one directory, the kept one, whose
/// revocation takes it back from what holds it once it holds no more.
pub(in crate::app) fn tool_trust(
    open: Option<Option<TrustedRoot>>,
    kept: Option<&TrustedRoot>,
) -> Option<TrustedRoot> {
    let Some(now) = open else {
        return kept.cloned();
    };
    let now = now?;
    let same = |kept: &&TrustedRoot| {
        !kept.revoked() && kept.path() == now.path() && kept.identity() == now.identity()
    };
    Some(kept.filter(same).cloned().unwrap_or(now))
}

impl App {
    /// The trust `caller`'s worktree jobs run under: that of an open project whose root is the
    /// session's, as it is now, else the one the session started with.
    pub(super) fn caller_trust(&self, caller: &Caller) -> Result<Trust, String> {
        let root = self.caller_root(caller).ok_or(
            "your session's project is not trusted; the person trusts it with trust-project",
        )?;
        Ok(Trust {
            root,
            trusted: self.trusted_list(),
        })
    }

    /// The trusted root of `caller`'s project: an open project's whose root is the session's,
    /// looked up now so that the person's later trust or distrust counts, else the one the
    /// session started with (a worktree just made, whose transcript's project is not found yet), while it holds.
    pub(super) fn caller_root(&self, caller: &Caller) -> Option<TrustedRoot> {
        if let Some(at) = &caller.acting_at {
            return Some(at.clone());
        }
        let kept = self.hosting.trusts.get(&caller.key);
        tool_trust(self.open_caller_root(caller), kept)
    }

    /// The trust a start in `caller`'s own directory is given, for its baseline: as its tools',
    /// but none a transcript alone gives, as a start given one is judged native otherwise than
    /// one given none, and no transcript decides that (ADR 0165).
    pub(super) fn caller_start_root(&self, caller: &Caller) -> Option<TrustedRoot> {
        let root = self.caller_root(caller)?;
        if caller.acting_at.is_some() {
            return Some(root);
        }
        let open = (caller.root.as_deref()).and_then(|root| self.open_root_trust(root));
        tool_trust(open, self.hosting.trusts.get(&caller.key))
    }

    /// The trust of the caller's project as an open buffer of it has it now: `None` when no
    /// buffer has it open, `Some(None)` when it is not trusted.
    fn open_caller_root(&self, caller: &Caller) -> Option<Option<TrustedRoot>> {
        caller
            .root
            .as_deref()
            .and_then(|root| self.open_trust_at(root))
    }

    /// Whether the caller's project is trusted now, by an open buffer of it, the session's own
    /// among them: never by the trust the session began with, which the person may have taken
    /// back since (ADR 0135).
    pub(super) fn caller_trusted_now(&self, caller: &Caller) -> bool {
        if let Some(at) = &caller.acting_at {
            return self.still_trusted(at);
        }
        self.open_caller_root(caller)
            .flatten()
            .is_some_and(|root| !root.revoked())
    }
}
