//! Stashes in the app (ADR 0127, step 3b): the stash menu's heads, the prompts they open for a
//! message or a stash when point is on none, the question before dropping one, and starting
//! each run on the repository's run thread.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or waits for
//! it: the repository's run thread, `oso-vcs-run`, finds the stash again by its id and runs.
//!
//! - A head acts on the stash at point; with none there, a prompt completes over the stashes
//!   the status shows, opened as its keys would open it. The prompt keeps the stashes it
//!   listed, so that an answer names the stash the person saw, whatever the status reads since.
//! - Dropping asks first, naming the stash; the `y` runs only for the status it was asked
//!   over, and a prompt or a question closed without its answer is forgotten.
//! - Applying and popping are refused while a merge, rebase, pick, revert or `git am` is
//!   stopped (Q6).
//! - What the echo area says names a stash by its subject, not its `stash@{N}`, which another
//!   worktree's push changes.

use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::complete::Candidate;
use oso_core::editor::{GitRequest, StashAction};
use oso_core::minibuffer::PromptId;

use super::super::App;
use super::runs::RUNNING;
use crate::commands::review::one_line;
use crate::commands::vcs::stash::STASHES_LIST;
use crate::vcs::history::Commit;
use crate::vcs::layout::{Built, Section};
use crate::vcs::run::{RunRequest, StashRun};
use crate::vcs::stash::{self, Act};

/// What a stash's prompt waits for, opened over the status `status`.
struct Prompted {
    status: BufferId,
    prompt: PromptId,
    reads: Reads,
}

/// What a stash's prompt reads.
enum Reads {
    /// A push's message, with untracked files if `untracked`.
    Message { untracked: bool },
    /// The stash to `act` on, of those `shown`.
    Stash(Act, Vec<Commit>),
}

/// A drop waiting for the `y` to the question `prompt`, asked over the status `status`.
struct Asked {
    status: BufferId,
    prompt: PromptId,
    stash: Commit,
}

/// A repository's stash prompt and question waiting.
#[derive(Default)]
pub(super) struct Stashes {
    prompt: Option<Prompted>,
    asked: Option<Asked>,
}

impl App {
    /// Carries out what the stash menu's heads asked.
    pub(super) fn git_stash_request(&mut self, request: GitRequest) {
        let GitRequest::Stash {
            buffer,
            action,
            line,
        } = request
        else {
            // The others are not a stash's (`requests::concern`).
            return;
        };
        let Some((project, root, built)) = self.trusted_status(buffer) else {
            return;
        };
        let index = self.git_repo(project, &root);
        if self.repo_busy(project) {
            return self.editor.message(RUNNING);
        }
        let stashes = &mut self.gits.commits.repos[index].stashes;
        let prompted = stashes.prompt.take().filter(|wait| wait.status == buffer);
        let asked = stashes.asked.take().filter(|wait| wait.status == buffer);
        let act = match action {
            StashAction::Push { untracked } => {
                let reads = Reads::Message { untracked };
                return self.open_stash_prompt(index, buffer, "git-stash-message", reads);
            }
            StashAction::Message(message) => {
                return self.stash_message(index, prompted.map(|wait| wait.reads), message);
            }
            StashAction::Picked(selector) => {
                let reads = prompted.map(|wait| wait.reads);
                return self.stash_picked(index, (&built, buffer), reads, &selector);
            }
            StashAction::Confirmed => {
                return match asked {
                    Some(Asked { stash, .. }) => {
                        let done = Act::Drop.done(&stash.subject);
                        let run = StashRun::Act {
                            act: Act::Drop,
                            id: stash.id,
                        };
                        self.run_stash(index, run, stash::title(Act::Drop), done);
                    }
                    None => self
                        .editor
                        .message("The drop asked about is gone: z k asks again"),
                };
            }
            StashAction::Apply => Act::Apply,
            StashAction::Pop => Act::Pop,
            StashAction::Drop => Act::Drop,
        };
        let shown = built.commits(Section::Stashes);
        match built.commit_at(line) {
            Some((Section::Stashes, stash)) => {
                let stash = stash.clone();
                self.stash_on(index, (&built, buffer), act, stash);
            }
            _ if shown.is_empty() => self.editor.message("There are no stashes"),
            _ => {
                let items: Arc<[Candidate]> = (shown.iter())
                    .map(|stash| {
                        let subject = String::from_utf8_lossy(&stash.subject);
                        Candidate::new(stash.short.as_str(), "", one_line(&subject, 80))
                    })
                    .collect();
                self.editor.set_listed(STASHES_LIST, items);
                let reads = Reads::Stash(act, shown.to_vec());
                self.open_stash_prompt(index, buffer, "git-stash-pick", reads);
            }
        }
    }

    /// The message a push's prompt read: stashes with it.
    fn stash_message(&mut self, index: usize, reads: Option<Reads>, message: String) {
        match reads {
            Some(Reads::Message { untracked }) => {
                let run = StashRun::Push { untracked, message };
                self.run_stash(index, run, "git stash push", "Stashed".to_owned());
            }
            _ => self
                .editor
                .message("The stash's prompt is gone: z z asks again"),
        }
    }

    /// The stash a prompt read as `selector`, found among those it listed.
    fn stash_picked(
        &mut self,
        index: usize,
        (built, status): (&Built, BufferId),
        reads: Option<Reads>,
        selector: &str,
    ) {
        let Some(Reads::Stash(act, shown)) = reads else {
            return self
                .editor
                .message("The stash's prompt is gone: z asks again");
        };
        let selector = selector.trim();
        match shown.into_iter().find(|stash| stash.short == selector) {
            Some(stash) => self.stash_on(index, (built, status), act, stash),
            None => self.editor.message(format!(
                "{} is not one of those listed",
                one_line(selector, 40)
            )),
        }
    }

    /// `act` on `stash`, one `built` showed: applying and popping are refused while something
    /// is stopped, and a drop asks first.
    fn stash_on(
        &mut self,
        index: usize,
        (built, status): (&Built, BufferId),
        act: Act,
        stash: Commit,
    ) {
        if act == Act::Drop {
            let question = stash::drop_question(&stash.short, &stash.subject);
            let yes = "git-stash-drop-confirmed";
            if let Some(prompt) = self.ask_bound(&question, yes, ["drop", "keep"]) {
                self.gits.commits.repos[index].stashes.asked = Some(Asked {
                    status,
                    prompt,
                    stash,
                });
            }
            return;
        }
        if self.refused_while_stopped(built) {
            return;
        }
        let done = act.done(&stash.subject);
        let run = StashRun::Act { act, id: stash.id };
        self.run_stash(index, run, stash::title(act), done);
    }

    /// Opens `command`'s prompt over the status `status`, and keeps it for its answer.
    fn open_stash_prompt(&mut self, index: usize, status: BufferId, command: &str, reads: Reads) {
        if let Some(prompt) = self.open_host_prompt(command) {
            self.gits.commits.repos[index].stashes.prompt = Some(Prompted {
                status,
                prompt,
                reads,
            });
        }
    }

    /// Starts `run`, called `title`, in the repository at `index`.
    fn run_stash(&mut self, index: usize, run: StashRun, title: &str, done: String) {
        let pass = self.pass_env();
        if self.ask_runner(index, RunRequest::Stash { run, pass }) {
            self.run_began(index, title.to_owned(), done);
        }
    }

    /// After the requests: forgets each stash's prompt or question that closed without its
    /// answer.
    pub(super) fn forget_closed_stash_prompts(&mut self) {
        let editor = &self.editor;
        let open = |id: PromptId| editor.prompts().iter().any(|prompt| prompt.id() == id);
        for repo in &mut self.gits.commits.repos {
            let stashes = &mut repo.stashes;
            stashes.prompt.take_if(|wait| !open(wait.prompt));
            stashes.asked.take_if(|asked| !open(asked.prompt));
        }
    }
}
