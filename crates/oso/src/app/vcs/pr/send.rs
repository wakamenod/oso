//! `x`, `X`, `c` and `C` in a pull request's buffer (ADR 0127, step 6b): what the line names, or
//! every failing check or unresolved thread, asked of the repository's `gh` thread, whose message
//! opens in `*review message*` if the person is still where they asked.
//!
//! Must not depend on: key presses or how a frame is drawn; `gh` runs on the repository's
//! thread, never here.

use oso_core::buffer::BufferId;
use oso_core::editor::PrAct;
use oso_core::project::ProjectId;

use super::super::super::App;
use super::Shows;
use crate::git::printable;
use crate::vcs::pr::send::Pick;
use crate::vcs::pr::work::{PrRequest, PrSent};
use crate::vcs::pr::write::PrRow;

impl App {
    /// What `act`, one of the sends, asks at `row` of the pull request's buffer `index`.
    pub(super) fn pr_send(&mut self, index: usize, act: PrAct, row: PrRow) {
        let view = &self.gits.prs.views[index];
        let (buffer, project, built) = (view.buffer, view.project, &view.built);
        let number = match (view.shows, &built.pull) {
            (Shows::Pull(_), Some((number, _))) => *number,
            _ => {
                return self.editor.message(
                    "In a pull request's buffer, x and X send its checks, c and C its threads",
                );
            }
        };
        let pick = match (act, row) {
            (PrAct::SendCheck, PrRow::Check(at)) => Pick::Check {
                name: built.check_names.get(at).cloned().unwrap_or_default(),
                link: built.links.get(at).cloned().unwrap_or_default(),
            },
            (PrAct::SendCheck, _) => {
                return self
                    .editor
                    .message("No check here: X sends the failing ones");
            }
            (PrAct::SendFailing, _) => Pick::Failing,
            (PrAct::SendThread, PrRow::Thread(at)) => match built.threads.get(at) {
                Some((path, line)) => Pick::Thread {
                    path: path.clone(),
                    line: *line,
                },
                None => return self.editor.message("No review thread here"),
            },
            (PrAct::SendThread, _) => {
                return self
                    .editor
                    .message("No review thread here: C sends the unresolved ones");
            }
            (PrAct::SendUnresolved, _) => Pick::Unresolved,
            _ => return,
        };
        self.gits.prs.generation += 1;
        let generation = self.gits.prs.generation;
        let request = PrRequest::Send {
            generation,
            number,
            pick,
        };
        if self.ask_pr(project, request).is_some() {
            self.gits.prs.send_asked = Some((generation, buffer));
            self.editor
                .echo(format!("Reading #{number} for the agent…"));
        }
    }

    /// A message for an agent session came: it opens in `*review message*`, if the person is
    /// still where they asked.
    pub(super) fn pr_sent(
        &mut self,
        project: ProjectId,
        generation: u64,
        sent: Result<Box<PrSent>, String>,
    ) {
        let asked = self.gits.prs.send_asked;
        let Some((_, from)) = asked.filter(|(asked, _)| *asked == generation) else {
            return;
        };
        self.gits.prs.send_asked = None;
        let sent = match sent {
            Ok(sent) => *sent,
            Err(why) => {
                return self
                    .editor
                    .message(printable(&why, super::super::MAX_MESSAGE));
            }
        };
        self.editor
            .clear_echo_saying(&format!("Reading #{} for the agent…", sent.number));
        if !self.still_at(from) {
            return self
                .editor
                .message("The message is read: ask again where you were");
        }
        self.open_pull_message(project, sent);
    }

    /// The buffer that shows pull request `number` of `project`, if one does.
    pub(in crate::app) fn pr_buffer_of(&self, project: ProjectId, number: u64) -> Option<BufferId> {
        (self.gits.prs.views.iter())
            .filter(|view| view.project == project && matches!(view.shows, Shows::Pull(_)))
            .find(|view| view.built.pull.as_ref().is_some_and(|(n, _)| *n == number))
            .map(|view| view.buffer)
            .filter(|buffer| self.editor.buffer(*buffer).is_some())
    }
}
