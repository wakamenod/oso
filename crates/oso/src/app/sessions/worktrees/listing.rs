//! `*Worktrees*` (ADR 0121, `worktree-list`): asking for a listing, and filling the buffer row by
//! row as the job looks at each linked worktree.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::Path;

use oso_core::buffer::{Buffer, Mode};

use super::super::super::App;
use super::{LIST, Listing};
use crate::review::TrustedRoot;
use crate::sessions::hosted::Run;
use crate::sessions::worktree::checks::Looked;
use crate::sessions::worktree::list::{self, Row, Shown};

impl App {
    /// `worktree-list`: `*Worktrees*`, filled as each worktree is looked at.
    pub(super) fn list_worktrees(&mut self) {
        let root = match self.list_root() {
            Ok(root) => root,
            Err(why) => return self.editor.message(why),
        };
        let kept = self.hosting.worktrees.list.as_ref().map(|list| list.buffer);
        let buffer =
            match kept.filter(|id| self.editor.buffer(*id).is_some_and(Buffer::is_internal)) {
                Some(id) => id,
                None => self.editor.add_internal_buffer(LIST, Mode::Worktrees),
            };
        let token = self.worktree_token();
        let shown = Shown {
            text: format!("Listing the worktrees of {}…\n", root.path().display()),
            rows: Vec::new(),
        };
        self.editor.set_internal_text(buffer, &shown.text);
        self.editor.show(buffer);
        let snapshot = self.worktree_snapshot();
        self.worktree_jobs().list(token, root.clone(), snapshot);
        self.hosting.worktrees.list = Some(Listing {
            token,
            buffer,
            root,
            rows: Vec::new(),
            pending: 0,
            shown,
        });
    }

    /// The repository `worktree-list` lists: the one shown in `*Worktrees*` from there, else
    /// the selected buffer's.
    pub(super) fn list_root(&self) -> Result<TrustedRoot, String> {
        let current = self.editor.window().buffer();
        match &self.hosting.worktrees.list {
            Some(list) if list.buffer == current => Ok(list.root.clone()),
            _ => self.trusted_here(),
        }
    }

    pub(super) fn worktrees_listed(&mut self, token: u64, result: Result<usize, String>) {
        let Some(list) = self
            .hosting
            .worktrees
            .list
            .as_mut()
            .filter(|l| l.token == token)
        else {
            return;
        };
        match result {
            Ok(count) => list.pending = count,
            Err(why) => {
                let text = format!("The worktrees cannot be listed: {why}\n");
                self.editor.set_internal_text(list.buffer, &text);
                return;
            }
        }
        self.show_worktree_rows();
    }

    pub(super) fn worktree_row(&mut self, token: u64, looked: Looked) {
        let snapshot = self.worktree_snapshot();
        let sessions = self.sessions_in(&looked.listed.path);
        let Some(list) = self
            .hosting
            .worktrees
            .list
            .as_mut()
            .filter(|l| l.token == token)
        else {
            return;
        };
        let held = snapshot.held(&looked.listed.path);
        list.rows.push(Row {
            looked,
            held,
            sessions,
        });
        list.pending = list.pending.saturating_sub(1);
        self.show_worktree_rows();
    }

    /// The hosted sessions in `path`, each as `name (state)`.
    pub(super) fn sessions_in(&self, path: &Path) -> Vec<String> {
        self.hosting
            .table
            .all()
            .iter()
            .filter(|session| session.directory.starts_with(path))
            .map(|session| {
                let state = match session.run {
                    Run::Starting | Run::Running { .. } => "running",
                    Run::Stopped => "stopped",
                    Run::Failed => "failed",
                };
                format!("{} ({state})", session.name)
            })
            .collect()
    }

    pub(super) fn show_worktree_rows(&mut self) {
        let Some(list) = self.hosting.worktrees.list.as_mut() else {
            return;
        };
        list.shown = list::render(list.root.path(), &list.rows, list.pending);
        let (buffer, text) = (list.buffer, list.shown.text.clone());
        self.editor.set_internal_text(buffer, &text);
    }
}
