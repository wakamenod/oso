//! `worktree_remove` for an agent (ADR 0121, Q14, the author's choice): no question once every
//! check holds, run in the job that removes the worktree, never with `--force`, and the branch
//! kept. The reply names what git ignored there, which went with it, and the person's echo area
//! says which session removed what.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::super::super::App;
use super::{Caller, Job, Pending, shown};
use crate::sessions::tools::Tool;
use crate::sessions::worktree::checks;
use crate::sessions::worktree::jobs::{Asked, Report};
use crate::sessions::worktree::porcelain::printable;

/// The most names of what git ignored a reply gives.
const NAMES: usize = 20;

impl App {
    /// `worktree_remove` of the worktree at `path`, from `caller`.
    pub(super) fn tool_worktree_remove(
        &mut self,
        pending: Pending,
        tool: &Tool,
        caller: &Caller,
        path: PathBuf,
    ) {
        if let Err(why) = self.admit_tool(tool, caller, &shown(&path)) {
            return pending.answer(Err(why));
        }
        let trust = match self.caller_trust(caller) {
            Ok(trust) => trust,
            Err(why) => return pending.answer(Err(why)),
        };
        if caller.directory.starts_with(&path) {
            return pending.answer(Err("that worktree is your own session's".into()));
        }
        // What the UI holds is looked at first, and the job looks at the rest.
        if let Some(check) = checks::held_refusal(&self.held_at(&path)) {
            return pending.answer(Err(format!("kept: {}", check.why())));
        }
        let token = self.worktree_token();
        let snapshot = self.worktree_snapshot();
        self.worktree_jobs()
            .remove(token, trust, path.clone(), snapshot, Asked::Unasked);
        self.hosting
            .tools
            .jobs
            .insert(token, Job::Removing { pending, path });
    }

    /// The removal of `path` ended.
    pub(super) fn tool_removed(&mut self, pending: Pending, path: &Path, report: Report) {
        let Report::Removed { result, .. } = report else {
            return pending.answer(Err("the removal ended unexpectedly".into()));
        };
        let place = printable(&shown(path), 200);
        let who = self
            .hosting
            .table
            .by_connection(pending.connection)
            .map_or_else(|| "an agent".to_owned(), |s| printable(&s.name, 40));
        match result {
            Ok(ignored) => {
                self.editor.message(format!(
                    "Session {who} removed worktree {place}; its branch is kept"
                ));
                self.agent_log("worktree_remove: removed");
                let names: Vec<String> = ignored
                    .iter()
                    .take(NAMES)
                    .map(|name| name.display().to_string())
                    .collect();
                pending.answer(Ok(json!({
                    "removed": shown(path),
                    "branch_kept": true,
                    "deleted_ignored": names,
                    "deleted_ignored_count": ignored.len(),
                })));
            }
            Err(why) => {
                self.agent_log("worktree_remove: kept");
                pending.answer(Err(format!("kept: {why}")));
            }
        }
    }
}
