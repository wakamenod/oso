//! `worktree-remove` (ADR 0121): the worktree to remove, its checks on a job, the question
//! that names what git ignores there, and the removal, never with `--force`, which keeps the
//! branch.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::{Path, PathBuf};

use super::super::super::App;
use super::{QUESTION_NAMES, Removing};
use crate::review::TrustedRoot;
use crate::sessions::worktree::checks::Check;
use crate::sessions::worktree::list;
use crate::sessions::worktree::porcelain::printable;

impl App {
    /// `worktree-remove`: the worktree at `path`, else the row at point in `*Worktrees*`, else
    /// the selected buffer's project, checked on a job first.
    pub(super) fn remove_worktree(&mut self, path: Option<PathBuf>) {
        let target = match path {
            Some(path) => self.trusted_here().map(|root| (root, path)),
            None => self.worktree_at_point(),
        };
        let (root, path) = match target {
            Ok(target) => target,
            Err(why) => return self.editor.message(why),
        };
        let token = self.worktree_token();
        let snapshot = self.worktree_snapshot();
        self.worktree_jobs()
            .check(token, root.clone(), path.clone(), snapshot);
        self.editor.message(format!(
            "Checking worktree {}…",
            printable(&path.display().to_string(), 200)
        ));
        self.hosting.worktrees.removing = Some(Removing {
            token,
            root,
            path,
            ignored: None,
        });
    }

    /// The worktree `worktree-remove` takes with no path given.
    pub(super) fn worktree_at_point(&self) -> Result<(TrustedRoot, PathBuf), String> {
        let (current, buffer) = (self.editor.window().buffer(), self.editor.current().1);
        if let Some(list) = self
            .hosting
            .worktrees
            .list
            .as_ref()
            .filter(|l| l.buffer == current)
        {
            let cursor = self.editor.window().selections.primary().cursor;
            let line = buffer.text().offset_to_point(cursor).row;
            return list::row_at(&list.shown, line)
                .map(|path| (list.root.clone(), path.to_owned()))
                .ok_or_else(|| "No worktree on this line".to_owned());
        }
        let root = self.trusted_here()?;
        let path = root.path().to_owned();
        Ok((root, path))
    }

    pub(super) fn worktree_checked(
        &mut self,
        token: u64,
        path: &Path,
        result: Result<Vec<PathBuf>, Check>,
    ) {
        let removing = self.hosting.worktrees.removing.as_mut();
        let Some(removing) = removing.filter(|r| r.token == token) else {
            return;
        };
        let shown = printable(&path.display().to_string(), 200);
        let ignored = match result {
            Ok(ignored) => ignored,
            Err(check) => {
                self.hosting.worktrees.removing = None;
                return self
                    .editor
                    .message(format!("Worktree {shown} kept: {}", check.why()));
            }
        };
        removing.ignored = Some(ignored.clone());
        if let Some(why) = self.held_refusal(path) {
            self.hosting.worktrees.removing = None;
            return self.editor.message(format!("Worktree {shown} kept: {why}"));
        }
        let names: Vec<String> = ignored
            .iter()
            .take(QUESTION_NAMES)
            .map(|name| printable(&name.display().to_string(), 60))
            .collect();
        let deletes = match ignored.len() {
            0 => String::new(),
            n if n > QUESTION_NAMES => format!(
                " It also deletes what git ignores there: {} and {} more.",
                names.join(", "),
                n - QUESTION_NAMES
            ),
            _ => format!(
                " It also deletes what git ignores there: {}.",
                names.join(", ")
            ),
        };
        let question = format!("Remove worktree {shown}? Its branch is kept.{deletes}");
        if let Err(e) = self.editor.ask(question, "worktree-remove-confirmed") {
            self.hosting.worktrees.removing = None;
            self.editor.message(e.to_string());
        }
    }

    /// Why what the UI thread holds keeps the worktree at `path` (checks 3 and 4), looked at
    /// again now.
    pub(super) fn held_refusal(&self, path: &Path) -> Option<String> {
        let held = self.worktree_snapshot().held(path);
        let looked_clean = Check::Unsaved(held.unsaved);
        if held.unsaved > 0 {
            return Some(looked_clean.why());
        }
        if held.unsent {
            return Some(Check::Unsent.why());
        }
        (!held.busy.is_empty()).then(|| Check::Busy(held.busy).why())
    }

    /// The person's `y`: the worktree is removed, if nothing changed that the UI holds.
    pub(super) fn remove_confirmed(&mut self) {
        let Some(removing) = self.hosting.worktrees.removing.take() else {
            return self.editor.message("No worktree waits to be removed");
        };
        if removing.ignored.is_none() {
            return self
                .editor
                .message("The worktree's checks have not finished");
        }
        let shown = printable(&removing.path.display().to_string(), 200);
        if let Some(why) = self.held_refusal(&removing.path) {
            return self.editor.message(format!("Worktree {shown} kept: {why}"));
        }
        let token = self.worktree_token();
        self.worktree_jobs()
            .remove(token, removing.root.clone(), removing.path.clone());
        self.hosting.worktrees.removing = Some(Removing { token, ..removing });
    }

    pub(super) fn worktree_removed(&mut self, token: u64, path: &Path, result: Result<(), String>) {
        let removing = self.hosting.worktrees.removing.take();
        if removing.is_none_or(|r| r.token != token) {
            return;
        }
        let shown = printable(&path.display().to_string(), 200);
        match result {
            Ok(()) => {
                self.editor
                    .message(format!("Removed worktree {shown}; its branch is kept"));
                let listing = self.hosting.worktrees.list.as_ref().map(|l| l.buffer);
                if listing == Some(self.editor.window().buffer()) {
                    self.list_worktrees();
                }
            }
            Err(why) => self.editor.message(format!("Worktree {shown} kept: {why}")),
        }
    }
}
