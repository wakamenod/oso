//! `*Worktrees*` (ADR 0121, `worktree-list`): asking for a listing, and filling the buffer row by
//! row as the job looks at each linked worktree, then with what `gh` says of the rows that
//! needed it. Each row is formatted once, and point stays on its line as rows come.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::Path;

use oso_core::buffer::{Buffer, Mode};
use oso_core::project::ProjectId;
use oso_core::selection::SelectionSet;

use super::super::super::App;
use super::{LIST, Listing, hosted_state};
use crate::sessions::worktree::checks::Looked;
use crate::sessions::worktree::list::{self, Row, Shown};
use crate::sessions::worktree::merged::Merged;

impl App {
    /// `worktree-list`: `*Worktrees*`, filled as each worktree is looked at.
    pub(super) fn list_worktrees(&mut self) {
        let (project, trust) = match self
            .list_project()
            .and_then(|id| self.trust_for(id).map(|trust| (id, trust)))
        {
            Ok(found) => found,
            Err(why) => return self.editor.message(why),
        };
        let kept = self.hosting.worktrees.list.as_ref().map(|list| list.buffer);
        let buffer =
            match kept.filter(|id| self.editor.buffer(*id).is_some_and(Buffer::is_internal)) {
                Some(id) => id,
                None => self.editor.add_internal_buffer(LIST, Mode::Worktrees),
            };
        let token = self.worktree_token();
        let main = trust.root.path().to_owned();
        let shown = Shown {
            text: format!("Listing the worktrees of {}…\n", main.display()),
            rows: Vec::new(),
        };
        self.editor.set_internal_text(buffer, &shown.text);
        self.editor.show(buffer);
        let snapshot = self.worktree_snapshot();
        self.worktree_jobs().list(token, trust, snapshot);
        self.hosting.worktrees.list = Some(Listing {
            token,
            buffer,
            project,
            main,
            rows: Vec::new(),
            lines: Vec::new(),
            pending: 0,
            shown,
        });
    }

    /// The project `worktree-list` lists: the one shown in `*Worktrees*` from there, else the
    /// selected buffer's.
    pub(super) fn list_project(&self) -> Result<ProjectId, String> {
        let current = self.editor.window().buffer();
        match &self.hosting.worktrees.list {
            Some(list) if list.buffer == current => Ok(list.project),
            _ => self.project_here(),
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
        if self
            .hosting
            .worktrees
            .list
            .as_ref()
            .is_none_or(|l| l.token != token)
        {
            return;
        }
        let held = self.held_at(&looked.listed.path);
        let sessions = self.sessions_in(&looked.listed.path);
        let space = self.space_of(&looked.listed.path);
        let Some(list) = self.hosting.worktrees.list.as_mut() else {
            return;
        };
        let row = Row {
            looked,
            held,
            sessions,
            space,
        };
        list.lines.push(list::row_lines(&list.main, &row));
        list.rows.push(row);
        list.pending = list.pending.saturating_sub(1);
        self.show_worktree_rows();
    }

    /// What `gh` said of the row at `path`: its merge, and its lines formatted again.
    pub(super) fn worktree_merged(&mut self, token: u64, path: &Path, merged: Merged) {
        let Some(list) = self
            .hosting
            .worktrees
            .list
            .as_mut()
            .filter(|l| l.token == token)
        else {
            return;
        };
        let Some(at) = list.rows.iter().position(|r| r.looked.listed.path == path) else {
            return;
        };
        list.rows[at].looked.merged = Some(merged);
        list.rows[at].looked.gh_pending = None;
        list.lines[at] = list::row_lines(&list.main, &list.rows[at]);
        self.show_worktree_rows();
    }

    /// The hosted sessions in `path`, each as `name (state)`: `running` while a running
    /// session's live state is not known.
    pub(in crate::app) fn sessions_in(&self, path: &Path) -> Vec<String> {
        self.hosting
            .table
            .all()
            .iter()
            .filter(|session| session.directory.starts_with(path))
            .map(|session| {
                let state =
                    hosted_state(session).map_or_else(|| "running".to_owned(), |s| s.to_string());
                format!("{} ({state})", session.name)
            })
            .collect()
    }

    /// Fills `*Worktrees*` with the rows so far, keeping point on its line where the selected
    /// window shows it.
    pub(super) fn show_worktree_rows(&mut self) {
        let Some(list) = self.hosting.worktrees.list.as_mut() else {
            return;
        };
        list.shown = list::assemble(&list.main, &list.lines, list.pending);
        let (buffer, text) = (list.buffer, list.shown.text.clone());
        let line = (self.editor.window().buffer() == buffer).then(|| {
            let (window, shown) = self.editor.current();
            shown
                .text()
                .offset_to_point(window.selections.primary().cursor)
                .row
        });
        self.editor.set_internal_text(buffer, &text);
        if let Some(line) = line {
            let (window, shown) = self.editor.current_mut();
            let rows = shown.text().line_count();
            let offset = shown.text().line_start(line.min(rows.saturating_sub(1)));
            window.selections = SelectionSet::new(offset);
            self.editor.follow_cursor();
        }
    }
}
