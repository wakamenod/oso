//! Issues in the app (ADR 0127, step 5b-1): what an issue buffer's keys and a list's ask, and
//! starting a worktree session from an issue, its brief made from it.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs `gh`: the
//! repository's `gh` thread does.
//!
//! - `w` asks the thread for the issue's brief and opens `*Brief*` with it, as
//!   `session-worktree` does, on the branch `issue-N-SLUG` from the remote's default branch,
//!   only if the person is still where they asked; they read it there before it is sent.
//! - Only the issue's own address opens, after the echo area shows it, and `y` copies only a
//!   plain `https` one.

use oso_core::buffer::BufferId;
use oso_core::editor::IssueAct;
use oso_core::project::ProjectId;

use super::super::super::App;
use super::PrBuffer;
use crate::git::printable;
use crate::vcs::pr::work::{IssueBrief, PrRequest};
use crate::vcs::pr::write::PrRow;

impl App {
    /// What `act` asks of `line` of the issue's buffer, or list of them, `buffer`.
    pub(super) fn issue_act(&mut self, buffer: BufferId, act: IssueAct, line: usize) {
        let Some(index) = (self.gits.prs.views.iter()).position(|view| view.buffer == buffer)
        else {
            return;
        };
        let view: &PrBuffer = &self.gits.prs.views[index];
        let project = view.project;
        // The issue on the line, in a list; the buffer's own, else.
        let at = match view.built.row(line) {
            PrRow::Pull(at) => view.built.pulls.get(at).cloned(),
            _ if !view.list => view.built.pull.clone(),
            _ => None,
        };
        match (act, at) {
            (IssueAct::Refresh, _) => self.read_pr(index),
            (IssueAct::Visit, Some((number, _))) if view.list => {
                self.show_github(project, (Some(number), false, true), false);
            }
            (IssueAct::Visit, _) => self.editor.message("Nothing here to visit"),
            (IssueAct::Open, Some((number, url))) => {
                self.editor
                    .message(format!("Opening {}", printable(&url, 300)));
                let request = PrRequest::Open {
                    url,
                    number,
                    issue: true,
                };
                self.ask_pr(project, request);
            }
            (IssueAct::CopyLink, Some((_, url))) => self.copy_pr_link(&url),
            (IssueAct::StartWorktree, Some((number, _))) => {
                self.gits.prs.generation += 1;
                let generation = self.gits.prs.generation;
                let request = PrRequest::IssueBrief { generation, number };
                if self.ask_pr(project, request).is_some() {
                    self.gits.prs.issue_brief = Some((generation, buffer));
                    self.editor
                        .echo(format!("Reading #{number} for its brief…"));
                }
            }
            (_, None) => self.editor.message("No issue here"),
        }
    }

    /// What a worktree session from an issue starts from came: `*Brief*` opens with it, if the
    /// person is still where they asked.
    pub(super) fn issue_briefed(
        &mut self,
        project: ProjectId,
        generation: u64,
        brief: Result<IssueBrief, String>,
    ) {
        let asked = self.gits.prs.issue_brief;
        let Some((_, from)) = asked.filter(|(asked, _)| *asked == generation) else {
            return;
        };
        self.gits.prs.issue_brief = None;
        let brief = match brief {
            Ok(brief) => brief,
            Err(why) => {
                return self
                    .editor
                    .message(printable(&why, super::super::MAX_MESSAGE));
            }
        };
        self.editor
            .clear_echo_saying(&format!("Reading #{} for its brief…", brief.number));
        if !self.still_at(from) {
            return self
                .editor
                .message("The issue's brief is read: w asks again");
        }
        self.start_brief(project, brief.branch, Some(brief.base), &brief.text);
    }
}
