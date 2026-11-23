//! The trust sessions start under, kept in `Hosting::trusts` for their agents' tools (ADR
//! 0121): judged as one is given to a start, and after each event, when what no longer holds
//! is taken back.
//!
//! Must not depend on: the disk; the editor's projects and trust list say it.
//!
//! - An open buffer of the root's project, the same directory, decides, trusted or not. One of
//!   another directory at its path takes it back while that is restricted, and else decides
//!   nothing. With none, the record does.
//! - Paths are compared as they are spelled, as a tool's call looks its project up.
//! - The record holds a root by path and identity, or the main repository its worktree's job
//!   found. A root made from a project holds too while the editor trusts that project, as a
//!   worktree through its main repository (ADR 0114).
//! - A record not read yet keeps a trust; one refused trusts nothing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use oso_core::editor::Editor;
use oso_core::project::ProjectId;

use super::{Baseline, Begin};
use crate::app::App;
use crate::git::TrustedRoot;
use crate::sessions::start::trust::{self, TrustRecord};
use crate::sessions::worker::SessionKey;

/// What the editor says of trust now: its open projects, by their roots' paths, and the record.
struct TrustNow<'a> {
    editor: &'a Editor,
    open: HashMap<&'a Path, Vec<ProjectId>>,
    record: TrustRecord<'a>,
}

impl<'a> TrustNow<'a> {
    fn of(editor: &'a Editor, record: TrustRecord<'a>) -> Self {
        let mut seen = HashSet::new();
        let mut open: HashMap<&Path, Vec<ProjectId>> = HashMap::new();
        for id in (editor.buffers()).filter_map(|(_, buffer)| buffer.project()) {
            if seen.insert(id)
                && let Some(project) = editor.project(id)
            {
                open.entry(project.root()).or_default().push(id);
            }
        }
        Self {
            editor,
            open,
            record,
        }
    }

    /// Whether an open buffer has `root` trusted: its own project, else none but a restricted
    /// one of another directory at its path.
    fn open_trust(&self, root: &TrustedRoot) -> Option<bool> {
        let at = self.open.get(root.path())?;
        let same = |id: &&ProjectId| {
            (self.editor.project(**id)).is_some_and(|project| project.identity() == root.identity())
        };
        match at.iter().find(same) {
            Some(&id) => Some(self.editor.project_trusted(id)),
            None => (at.iter().any(|&id| !self.editor.project_trusted(id))).then_some(false),
        }
    }

    /// Whether the trust of `root` holds.
    fn holds(&self, root: &TrustedRoot) -> bool {
        !root.revoked()
            && trust::stands(self.open_trust(root), self.record, |list| {
                root.listed_in(list) || root.project_trusted(self.editor)
            })
    }

    /// The trust given to a start, apart from what else holds it, so that each is taken back by
    /// itself. A start given none is not judged here.
    ///
    /// # Errors
    ///
    /// That it does not hold, for the echo area.
    fn judge_given(
        &self,
        trusted: Option<TrustedRoot>,
    ) -> Result<Option<TrustedRoot>, &'static str> {
        match trusted {
            Some(root) if !self.holds(&root) => {
                Err("The session did not start: its project is no longer trusted")
            }
            trusted => Ok(trusted.map(TrustedRoot::apart)),
        }
    }

    /// Takes back and forgets each of `trusts` that does not hold.
    fn sweep(&self, trusts: &mut HashMap<SessionKey, TrustedRoot>) {
        trusts.retain(|_, root| {
            let holds = self.holds(root);
            if !holds {
                root.revoke();
            }
            holds
        });
    }
}

impl App {
    /// Whether the trust of `root` holds now.
    pub(in crate::app) fn trust_holds(&self, root: &TrustedRoot) -> bool {
        TrustNow::of(&self.editor, self.trust_record()).holds(root)
    }

    /// The trust a new session is given as it stands now. What the session starts under, this
    /// or what `start_trust` judges where it runs, goes in `Hosting::trusts` in the same event.
    ///
    /// # Errors
    ///
    /// Why the session does not start.
    pub(super) fn judge_given_trust(
        &self,
        trusted: Option<TrustedRoot>,
    ) -> Result<Option<TrustedRoot>, &'static str> {
        TrustNow::of(&self.editor, self.trust_record()).judge_given(trusted)
    }

    /// The trust and root new session `key` starts under in `directory`, natively if
    /// `native`: a native start's as `native_start` judged it, a worktree's just made, an
    /// agent's terminal start under its session's, else an open trusted project's, whose
    /// baseline it takes.
    pub(super) fn start_trust(
        &mut self,
        key: SessionKey,
        begin: &Begin,
        directory: &Path,
        project: Option<ProjectId>,
        native: bool,
    ) -> (Option<TrustedRoot>, Option<PathBuf>) {
        match begin.trusted.clone() {
            // A native start under an agent's trust runs under the one `native_start` judged
            // where it runs, which the start thread looks at again (ADR 0165, Q5).
            Some(_) if native && !begin.worktree => {
                let judged = self.native_trust_now(directory, Some(directory)).ok();
                (judged, Some(directory.to_path_buf()))
            }
            // A worktree just made, which no buffer opened yet: its own root and trust; or an
            // agent's start in a terminal, under its session's trust.
            Some(trusted) => (Some(trusted), Some(directory.to_path_buf())),
            None => {
                let trusted = project.and_then(|id| TrustedRoot::of(&self.editor, id));
                if let (Some(project), Some(root)) = (project, &trusted) {
                    let baseline = Baseline {
                        project,
                        root: root.clone(),
                        to_review: true,
                    };
                    self.hosting.baselines.insert(key, baseline);
                }
                let root = project
                    .and_then(|id| self.editor.project(id))
                    .map(|project| project.root().to_path_buf());
                (trusted, root)
            }
        }
    }

    /// Takes back the trust each session started under that no longer holds. After each event.
    /// The open projects are looked at once, whatever the number of sessions.
    pub(super) fn revoke_untrusted_session_trusts(&mut self) {
        if self.hosting.trusts.is_empty() {
            return;
        }
        let mut trusts = std::mem::take(&mut self.hosting.trusts);
        TrustNow::of(&self.editor, self.trust_record()).sweep(&mut trusts);
        self.hosting.trusts = trusts;
    }
}

#[cfg(test)]
mod tests;
