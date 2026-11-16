//! A status buffer's own requests in the app (ADR 0127): reading it again, folding, staging and
//! unstaging what its lines name, and visiting the file or the commit a line is about.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git: the status's
//! thread, `oso-vcs`, is asked, and what a line names comes from the table built with the text.

use oso_core::buffer::BufferId;
use oso_core::editor::{GitLines, GitRequest};

use super::super::App;
use super::Ask;
use crate::git::os_path;
use crate::vcs::stage;

impl App {
    /// Carries out what the status's own commands asked.
    pub(super) fn git_status_request(&mut self, request: GitRequest) {
        match request {
            GitRequest::Status { project } => self.open_git_status(project),
            GitRequest::Refresh { buffer } => {
                if let Some(index) = self.git_index(buffer) {
                    self.git_ask(index, Ask::Read);
                }
            }
            GitRequest::Toggle { buffer, line } => self.toggle_git(buffer, line),
            GitRequest::Stage { buffer, lines } => self.stage_git(buffer, &lines, true),
            GitRequest::Unstage { buffer, lines } => self.stage_git(buffer, &lines, false),
            GitRequest::StageAll { buffer } => self.stage_git_all(buffer, true),
            GitRequest::UnstageAll { buffer } => self.stage_git_all(buffer, false),
            GitRequest::Visit {
                buffer,
                line,
                column,
            } => self.visit_git_line(buffer, line, column),
            // The others are not the status's own (`requests::concern`).
            _ => {}
        }
    }

    /// Folds or unfolds the section at `line`, or shows or hides the hunks of the file there;
    /// on a hunk, hides its file's hunks.
    fn toggle_git(&mut self, buffer: BufferId, line: usize) {
        let Some(index) = self.git_index(buffer) else {
            return;
        };
        let open = &mut self.gits.open[index];
        if open.view.toggle(&open.built, line) {
            self.git_ask(index, Ask::Layout);
        }
    }

    fn stage_git(&mut self, buffer: BufferId, lines: &[GitLines], stage: bool) {
        let Some(index) = self.git_index(buffer) else {
            return;
        };
        match stage::targets(&self.gits.open[index].built, lines) {
            Ok(targets) if targets.is_empty() => self.editor.message("Nothing here to stage"),
            Ok(targets) => self.git_ask(index, Ask::Stage(targets, stage)),
            Err(why) => self.editor.message(why),
        }
    }

    fn stage_git_all(&mut self, buffer: BufferId, stage: bool) {
        let Some(index) = self.git_index(buffer) else {
            return;
        };
        match stage::all(&self.gits.open[index].built, stage) {
            Ok(target) => self.git_ask(index, Ask::Stage(vec![target], stage)),
            Err(why) => self.editor.message(why),
        }
    }

    /// Visits the file `line` is about, at the line of it a hunk's line shows.
    fn visit_git_line(&mut self, buffer: BufferId, line: usize, column: usize) {
        let Some(index) = self.git_index(buffer) else {
            return;
        };
        let open = &self.gits.open[index];
        // A commit's row opens its review, read-only (step 2).
        if let Some((_, commit)) = open.built.commit_at(line) {
            let (project, id) = (open.project, commit.id.clone());
            return self.open_review(project, oso_core::editor::ScopeKind::Commit, Some(id));
        }
        let visit = match open.built.visit(line, column) {
            Ok(visit) => visit,
            Err(why) => return self.editor.message(why),
        };
        let Ok(relative) = os_path(visit.path) else {
            return self
                .editor
                .message("That file's name is not one this system can open");
        };
        let path = open.root.path().join(relative);
        match visit.at {
            Some((line, column)) => {
                self.editor
                    .request_visit_at(path, oso_core::load::UNASKED, line, column);
            }
            None => self.editor.request_visit(path, oso_core::load::UNASKED),
        }
        self.serve_files();
    }
}
