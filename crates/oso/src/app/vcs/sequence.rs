//! What is stopped, in the app (ADR 0127, step 3a): `C-c C-c` continuing a merge, rebase, pick,
//! revert or `git am`, and the questions before `C-c C-s` skips a commit and `C-c C-k` aborts,
//! each starting its run on the repository's run thread. The code calls these a sequence, as
//! magit does, apart from a run the person stops with `C-g`.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or waits for
//! it: the repository's run thread, `oso-vcs-run`, judges what is stopped again and runs the
//! step, and reports here.
//!
//! - A skip or an abort asks first, naming what goes (Q2, Q3). The `y` runs only for the status
//!   it was asked over, and one closed with `n`, `C-g` or another prompt over it is forgotten;
//!   the run thread refuses it unless what is stopped is still what the question was asked over.
//! - A continue is refused while unmerged paths are left, naming `s`.
//! - While a merge, rebase, pick, revert or `git am` is stopped, the commit menu and a pull are
//!   refused, naming these keys (Q6).

use oso_core::buffer::BufferId;
use oso_core::editor::GitRequest;
use oso_core::minibuffer::PromptId;

use super::super::App;
use super::runs::RUNNING;
use crate::vcs::layout::{Built, Section};
use crate::vcs::run::RunRequest;
use crate::vcs::sequence::{self, Doing, Step, Want};

/// A skip or an abort waiting for the `y` to the question `prompt`, asked over the status
/// `status`.
pub(super) struct SequenceAsked {
    status: BufferId,
    prompt: PromptId,
    want: Want,
}

impl App {
    /// Carries out what `C-c C-c`, `C-c C-s` and `C-c C-k` asked.
    pub(super) fn git_sequence_request(&mut self, request: GitRequest) {
        match request {
            GitRequest::Continue { buffer } => self.step_git(buffer, Step::Continue, false),
            GitRequest::Skip { buffer, confirmed } => self.step_git(buffer, Step::Skip, confirmed),
            GitRequest::Abort { buffer, confirmed } => {
                self.step_git(buffer, Step::Abort, confirmed);
            }
            // The others are not a sequence's (`requests::concern`).
            _ => {}
        }
    }

    fn step_git(&mut self, buffer: BufferId, step: Step, confirmed: bool) {
        let Some((project, root, built)) = self.trusted_status(buffer) else {
            return;
        };
        let index = self.git_repo(project, &root);
        if self.repo_busy(project) {
            return self.editor.message(RUNNING);
        }
        if confirmed {
            return match self.gits.commits.repos[index].sequence_asked.take() {
                Some(wait) if wait.status == buffer && wait.want.step == step => {
                    self.run_step(index, wait.want);
                }
                _ => self
                    .editor
                    .message(format!("The question is gone: {} asks again", step.key())),
            };
        }
        let Some(stop) = built.stop.as_ref() else {
            return self.editor.message("Nothing is stopped here");
        };
        if let Some(why) = stop.doing.refuses(step) {
            return self.editor.message(why);
        }
        let want = Want::new(step, stop);
        if step == Step::Continue {
            let left = built.files(Section::Unmerged).len();
            if left > 0 {
                let paths = if left == 1 { "path is" } else { "paths are" };
                return self.editor.message(format!(
                    "{left} unmerged {paths} left: resolve each, s stages it"
                ));
            }
            return self.run_step(index, want);
        }
        let (question, yes, verbs) = match step {
            Step::Skip => (stop.skip_question(), "git-skip-confirmed", ["skip", "keep"]),
            _ => (
                stop.abort_question(),
                "git-abort-confirmed",
                ["abort", "keep"],
            ),
        };
        if let Some(prompt) = self.ask_bound(&question, yes, verbs) {
            self.gits.commits.repos[index].sequence_asked = Some(SequenceAsked {
                status: buffer,
                prompt,
                want,
            });
        }
    }

    /// Starts `want` in the repository at `index`.
    fn run_step(&mut self, index: usize, want: Want) {
        let title = sequence::title(want.stop.doing, want.step);
        let done = sequence::done(want.stop.doing, want.step);
        let pass = self.pass_env();
        if self.ask_runner(index, RunRequest::Step { want, pass }) {
            self.run_began(index, title, done);
        }
    }

    /// After the requests: forgets each question before a skip or an abort that closed without
    /// its answer.
    pub(super) fn forget_closed_sequence_questions(&mut self) {
        let editor = &self.editor;
        let open = |id: PromptId| editor.prompts().iter().any(|prompt| prompt.id() == id);
        for repo in &mut self.gits.commits.repos {
            if (repo.sequence_asked.as_ref()).is_some_and(|wait| !open(wait.prompt)) {
                repo.sequence_asked = None;
            }
        }
    }

    /// Whether `built` shows a merge, rebase, pick, revert or `git am` stopped, said in the echo
    /// area: what the commit menu and a pull are refused for (Q6).
    pub(super) fn refused_while_stopped(&mut self, built: &Built) -> bool {
        let Some(stop) = built
            .stop
            .as_ref()
            .filter(|stop| stop.doing != Doing::Bisect)
        else {
            return false;
        };
        self.editor.message(format!(
            "A {} is stopped here: {} continues it, {} aborts it",
            stop.doing.noun(),
            Step::Continue.key(),
            Step::Abort.key()
        ));
        true
    }
}
