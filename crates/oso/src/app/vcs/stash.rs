//! Stashes in the app (ADR 0127, step 3b): the stash menu's heads, the prompts they open for a
//! message or a stash when point is on none, the question before dropping one, and starting
//! each run on the repository's run thread.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or waits for
//! it: the repository's run thread, `oso-vcs-run`, finds the stash again by its id and runs.
//!
//! - A head acts on the stash at point; with none there, a prompt completes over the stashes
//!   the status shows, opened as its keys would open it.
//! - Dropping asks first, naming the stash; the `y` runs only for the status it was asked
//!   over, and a prompt or a question closed without its answer is forgotten.
//! - Applying and popping are refused while a merge, rebase, pick, revert or `git am` is
//!   stopped (Q6).

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
#[derive(Clone, Copy)]
enum Reads {
    /// A push's message, with untracked files if `untracked`.
    Message { untracked: bool },
    /// The stash to `act` on.
    Stash(Act),
}

/// A drop waiting for the `y` to the question `prompt`, asked over the status `status`.
struct Asked {
    status: BufferId,
    prompt: PromptId,
    id: String,
    selector: String,
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
        stashes.asked.take_if(|_| action != StashAction::Confirmed);
        let prompted = stashes.prompt.take();
        match action {
            StashAction::Push { untracked } => {
                self.open_stash_prompt(index, buffer, Reads::Message { untracked });
            }
            StashAction::Message(message) => match prompted {
                Some(Prompted {
                    status,
                    reads: Reads::Message { untracked },
                    ..
                }) if status == buffer => {
                    let run = StashRun::Push { untracked, message };
                    self.run_stash(index, run, "git stash push", "Stashed".to_owned());
                }
                _ => self
                    .editor
                    .message("The stash's prompt is gone: z z asks again"),
            },
            StashAction::Apply => self.stash_at(index, (&*built, buffer), Act::Apply, line),
            StashAction::Pop => self.stash_at(index, (&*built, buffer), Act::Pop, line),
            StashAction::Drop => self.stash_at(index, (&*built, buffer), Act::Drop, line),
            StashAction::Picked(selector) => match prompted {
                Some(Prompted {
                    status,
                    reads: Reads::Stash(act),
                    ..
                }) if status == buffer => match stash_named(&built, &selector) {
                    Some(commit) => self.stash_on(index, (&*built, buffer), act, commit),
                    None => self.editor.message(format!("No stash {selector} is shown")),
                },
                _ => self
                    .editor
                    .message("The stash's prompt is gone: z asks again"),
            },
            StashAction::Confirmed => match self.gits.commits.repos[index].stashes.asked.take() {
                Some(asked) if asked.status == buffer => {
                    let done = Act::Drop.done(&asked.selector);
                    let title = stash::title(Act::Drop, &asked.selector);
                    let run = StashRun::On {
                        act: Act::Drop,
                        id: asked.id,
                    };
                    self.run_stash(index, run, &title, done);
                }
                _ => self
                    .editor
                    .message("The drop asked about is gone: z k asks again"),
            },
        }
    }

    /// `act` on the stash at `line` of `built`, the status `status`'s, or one read in a prompt
    /// if none is there.
    fn stash_at(
        &mut self,
        index: usize,
        (built, status): (&Built, BufferId),
        act: Act,
        line: usize,
    ) {
        if act != Act::Drop && self.refused_while_stopped(built) {
            return;
        }
        match built.commit_at(line) {
            Some((Section::Stashes, commit)) => {
                let commit = commit.clone();
                self.stash_on(index, (built, status), act, &commit);
            }
            _ if stashes_of(built).is_empty() => self.editor.message("There are no stashes"),
            _ => self.open_stash_prompt(index, status, Reads::Stash(act)),
        }
    }

    /// `act` on `commit`, a stash `built` shows: a drop asks first.
    fn stash_on(
        &mut self,
        index: usize,
        (built, status): (&Built, BufferId),
        act: Act,
        commit: &Commit,
    ) {
        if act != Act::Drop {
            if self.refused_while_stopped(built) {
                return;
            }
            let title = stash::title(act, &commit.short);
            let run = StashRun::On {
                act,
                id: commit.id.clone(),
            };
            return self.run_stash(index, run, &title, act.done(&commit.short));
        }
        let question = one_line(&stash::drop_question(&commit.short, &commit.subject), 400);
        match (self.editor).ask_with(
            format!("{question} (y or n)"),
            "git-stash-drop-confirmed",
            ["drop", "keep"],
        ) {
            Ok(prompt) => {
                self.gits.commits.repos[index].stashes.asked = Some(Asked {
                    status,
                    prompt,
                    id: commit.id.clone(),
                    selector: commit.short.clone(),
                });
            }
            Err(e) => self.editor.message(format!("Cannot ask: {e}")),
        }
        self.request_redraw();
    }

    /// Opens the prompt that reads what `reads` says over the status `status`, as its keys
    /// would, and keeps it for its answer.
    fn open_stash_prompt(&mut self, index: usize, status: BufferId, reads: Reads) {
        let command = match reads {
            Reads::Message { .. } => "git-stash-message",
            Reads::Stash(_) => {
                let built = (self.gits.open.iter())
                    .find(|open| open.buffer == status)
                    .map(|open| Arc::clone(&open.built));
                let items: Arc<[Candidate]> = (built.iter())
                    .flat_map(|built| stashes_of(built).to_vec())
                    .map(|commit| {
                        let subject = String::from_utf8_lossy(&commit.subject).into_owned();
                        Candidate::new(commit.short.as_str(), "", one_line(&subject, 80))
                    })
                    .collect();
                self.editor.set_listed(STASHES_LIST, items);
                "git-stash-pick"
            }
        };
        let before = self.editor.prompt().map(oso_core::minibuffer::Prompt::id);
        let effects = self.dispatcher.run_from_host(&mut self.editor, command);
        // Only a prompt this opened takes the answer.
        let opened = self.editor.prompt().map(oso_core::minibuffer::Prompt::id);
        if let Some(prompt) = opened.filter(|id| Some(*id) != before) {
            self.gits.commits.repos[index].stashes.prompt = Some(Prompted {
                status,
                prompt,
                reads,
            });
        }
        if effects.redraw {
            self.request_redraw();
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

/// The stashes `built` shows, newest first.
fn stashes_of(built: &Built) -> &[Commit] {
    (built.logs.iter())
        .find(|(section, _)| *section == Section::Stashes)
        .map_or(&[], |(_, log)| log.commits.as_slice())
}

/// The stash `built` shows as `selector`.
fn stash_named<'a>(built: &'a Built, selector: &str) -> Option<&'a Commit> {
    let selector = selector.trim();
    stashes_of(built)
        .iter()
        .find(|commit| commit.short == selector)
}
