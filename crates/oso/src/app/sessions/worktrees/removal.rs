//! `worktree-remove` (ADR 0121): the worktree to remove, its checks on a job, the question
//! that names what git ignores there, and the removal, never with `--force`, which keeps the
//! branch. The person's `y` runs every check again, in the job that removes it; once they hold,
//! the worktree's idle sessions are stopped and its space closed before git removes it.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::{Path, PathBuf};

use oso_core::project::ProjectId;

use super::super::super::App;
use super::{QUESTION_NAMES, Removing, running_sessions};
use crate::sessions::worktree::checks::{self, Check};
use crate::sessions::worktree::jobs::Go;
use crate::sessions::worktree::list;
use crate::sessions::worktree::porcelain::printable;

impl App {
    /// `worktree-remove`: the worktree at `path`, else the row at point in `*Worktrees*`, else
    /// the selected buffer's project, checked on a job first.
    pub(super) fn remove_worktree(&mut self, path: Option<PathBuf>) {
        let target = match path {
            Some(path) => self.project_here().map(|project| (project, path)),
            None => self.worktree_at_point(),
        };
        let (project, path, trust) = match target
            .and_then(|(project, path)| Ok((project, path, self.trust_for(project)?)))
        {
            Ok(target) => target,
            Err(why) => return self.editor.message(why),
        };
        let token = self.worktree_token();
        let snapshot = self.worktree_snapshot();
        self.worktree_jobs()
            .check(token, trust, path.clone(), snapshot);
        self.editor.message(format!(
            "Checking worktree {}…",
            printable(&path.display().to_string(), 200)
        ));
        self.hosting.worktrees.removing = Some(Removing {
            token,
            project,
            path,
            ignored: None,
        });
    }

    /// The worktree `worktree-remove` takes with no path given.
    pub(super) fn worktree_at_point(&self) -> Result<(ProjectId, PathBuf), String> {
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
                .map(|path| (list.project, path.to_owned()))
                .ok_or_else(|| "No worktree on this line".to_owned());
        }
        let project = self.project_here()?;
        let root = self
            .editor
            .project(project)
            .map(|found| found.root().to_owned())
            .ok_or("This buffer's project is gone")?;
        Ok((project, root))
    }

    pub(super) fn worktree_checked(
        &mut self,
        token: u64,
        path: &Path,
        result: Result<Vec<PathBuf>, Check>,
    ) {
        if self
            .hosting
            .worktrees
            .removing
            .as_ref()
            .is_none_or(|r| r.token != token)
        {
            return;
        }
        let shown = printable(&path.display().to_string(), 200);
        // What the UI holds is looked at again now, as it may have changed during the checks.
        let checked = result.and_then(|ignored| match checks::held_refusal(&self.held_at(path)) {
            Some(check) => Err(check),
            None => Ok(ignored),
        });
        let ignored = match checked {
            Ok(ignored) => ignored,
            Err(check) => {
                self.hosting.worktrees.removing = None;
                return self
                    .editor
                    .message(format!("Worktree {shown} kept: {}", check.why()));
            }
        };
        let question = question(&shown, &ignored);
        if let Some(removing) = self.hosting.worktrees.removing.as_mut() {
            removing.ignored = Some(ignored);
        }
        if let Err(e) = self.editor.ask(question, "worktree-remove-confirmed") {
            self.hosting.worktrees.removing = None;
            self.editor.message(e.to_string());
        }
    }

    /// The person's `y`: the worktree is removed by a job that runs every check again first,
    /// if nothing changed that the UI holds.
    pub(super) fn remove_confirmed(&mut self) {
        let Some(removing) = self.hosting.worktrees.removing.take() else {
            return self.editor.message("No worktree waits to be removed");
        };
        let Some(asked) = removing.ignored.as_ref().map(Vec::len) else {
            return self
                .editor
                .message("The worktree's checks have not finished");
        };
        let shown = printable(&removing.path.display().to_string(), 200);
        if let Some(check) = checks::held_refusal(&self.held_at(&removing.path)) {
            return self
                .editor
                .message(format!("Worktree {shown} kept: {}", check.why()));
        }
        let trust = match self.trust_for(removing.project) {
            Ok(trust) => trust,
            Err(why) => return self.editor.message(why),
        };
        let token = self.worktree_token();
        let snapshot = self.worktree_snapshot();
        self.worktree_jobs()
            .remove(token, trust, removing.path.clone(), snapshot, asked);
        self.editor.message(format!("Removing worktree {shown}…"));
        self.hosting.worktrees.removing = Some(Removing { token, ..removing });
    }

    /// The checks held again in the job that removes the worktree: what the UI holds is looked
    /// at once more, then its idle sessions are stopped and its space closed before the job
    /// goes on. Not saying [`Go`] calls the removal off.
    pub(super) fn worktree_clear(&mut self, token: u64, path: &Path, go: Go) {
        if self
            .hosting
            .worktrees
            .removing
            .as_ref()
            .is_none_or(|r| r.token != token)
        {
            return;
        }
        if let Some(check) = checks::held_refusal(&self.held_at(path)) {
            self.hosting.worktrees.removing = None;
            let shown = printable(&path.display().to_string(), 200);
            return self
                .editor
                .message(format!("Worktree {shown} kept: {}", check.why()));
        }
        self.stop_idle_sessions_in(path);
        // ADR 0121 step 3 part B: close the worktree's space here, entering its repository's
        // first if it is the one in the frame. `Spaces` has no way to close one yet.
        go.proceed();
    }

    /// Stops the idle hosted sessions in the worktree at `path`, which check 4 let through:
    /// their buffers stay, read-only, as a stopped session's do.
    fn stop_idle_sessions_in(&mut self, path: &Path) {
        let idle: Vec<String> = running_sessions(self.hosting.table.all())
            .into_iter()
            .filter(|session| session.settled && session.directory.starts_with(path))
            .map(|session| session.name)
            .collect();
        // ADR 0121 step 3 part B: stop each one as `session-stop` does, as ended by oso so
        // that it becomes stopped, not failed, and keeps its buffer. Nothing stops a hosted
        // session so yet: `C-x k` archives it with its buffer, and a hang-up alone ends by a
        // signal, which reads as failed. None is idle here yet either: until the app keeps
        // live states, a running session is busy and check 4 refused it.
        if !idle.is_empty() {
            log::warn!(
                "{} idle sessions are not stopped before a removal",
                idle.len()
            );
        }
    }

    pub(super) fn worktree_removed(&mut self, token: u64, path: &Path, result: Result<(), String>) {
        if self
            .hosting
            .worktrees
            .removing
            .as_ref()
            .is_none_or(|r| r.token != token)
        {
            return;
        }
        self.hosting.worktrees.removing = None;
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

/// The removal's question for the worktree shown as `shown`, naming what git ignores there.
fn question(shown: &str, ignored: &[PathBuf]) -> String {
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
    format!("Remove worktree {shown}? Its branch is kept.{deletes}")
}
