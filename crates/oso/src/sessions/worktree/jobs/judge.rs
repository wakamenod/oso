//! Judging at a session's turn's end whether the changes it left are merged (ADR 0121, "Merged
//! work is not ready for review, as amended"): a short job ahead of those waiting.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread.

use super::{Jobs, Report, Trust};
use crate::git::Id;
use crate::sessions::worker::SessionKey;
use crate::sessions::worktree::merged;

impl Jobs {
    /// Judges whether the work of session `key` at its trusted root is merged, for the app's
    /// judgment `judgment`: `tree` is what its review snapshot took there at its turn's end.
    /// Ancestry only; no `gh`. It goes ahead of the jobs waiting, as it is short and a state
    /// waits for it. A root this job cannot run git in is not merged, so the session is ready
    /// for review as before.
    pub fn work_merged(&self, key: SessionKey, judgment: u64, trust: Trust, tree: Id) {
        self.on_repo(
            trust,
            true,
            move |root, repo, send| {
                let merged = merged::work_merged(repo, root.path(), &tree).is_merged();
                send(Report::WorkMerged {
                    key,
                    judgment,
                    merged,
                });
            },
            move |_| Report::WorkMerged {
                key,
                judgment,
                merged: false,
            },
        );
    }
}
