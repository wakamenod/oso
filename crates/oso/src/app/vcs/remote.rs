//! Pushes, pulls and fetches in the app (ADR 0127, step 2): where a push goes, the prompt it
//! opens when the branch has no target, the question before a forced one, and starting each run
//! on the repository's run thread.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or waits for
//! it: the repository's run thread, `oso-vcs-run`, plans each push and runs it, and reports
//! here.
//!
//! - A push is planned on the run thread first, from git's own configuration (Q4): with no
//!   target, the remotes go to a prompt (`git-push-to-remote`), and `e`'s remote branches to
//!   another that takes any name (`git-push-to`), each opened as its keys would open it.
//! - A forced push asks first, naming the commits it would drop; the `y` runs what was asked,
//!   with the lease read as it was planned, so that git refuses it if the remote moved since.
//! - One run at a time in a repository, a plan waiting counted as one.

use std::sync::Arc;
use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_core::complete::Candidate;
use oso_core::editor::{FetchFrom, GitRequest, PushSwitches, PushTarget};
use oso_core::minibuffer::PromptId;

use super::super::App;
use super::runs::{MAX_SAID, RUNNING, Running};
use crate::commands::review::one_line;
use crate::commands::vcs::remote::{REMOTES_LIST, TARGETS_LIST};
use crate::vcs::remote::{PULL, Push, fetch_title};
use crate::vcs::run::{Planned, RemoteRun, RunRequest, Want};

/// A push being planned, asked from the status `status`.
pub(super) struct Planning {
    status: BufferId,
    switches: PushSwitches,
}

/// A push waiting for the answer to the prompt `prompt` it opened over the status `status`.
pub(super) struct Prompted {
    status: BufferId,
    prompt: PromptId,
    switches: PushSwitches,
    reads: Reads,
}

/// A forced push waiting for the `y` to the question `prompt`, asked over the status `status`.
pub(super) struct Asked {
    status: BufferId,
    prompt: PromptId,
    push: Push,
}

/// What a push's prompt reads.
#[derive(Clone, Copy)]
enum Reads {
    /// A remote, to the branch's own name there, set as the upstream if `set_upstream`.
    Remote { set_upstream: bool },
    /// `REMOTE/BRANCH`.
    Branch,
}

/// A repository's pushes waiting: one being planned, its prompt, and the question asked. A
/// prompt or a question closed without its answer is forgotten, so that nothing runs what was
/// refused, and an answer runs only for the status it was asked over.
#[derive(Default)]
pub(super) struct Pushes {
    pub(super) planning: Option<Planning>,
    prompt: Option<Prompted>,
    asked: Option<Asked>,
}

impl App {
    /// Carries out what the push, pull and fetch menus' heads asked.
    pub(super) fn git_remote_request(&mut self, request: GitRequest) {
        match request {
            GitRequest::Push {
                buffer,
                target,
                switches,
            } => self.push_git(buffer, target, switches),
            GitRequest::Pull { buffer } => self.pull_git(buffer),
            GitRequest::Fetch { buffer, from } => self.fetch_git(buffer, from),
            // The others are not a remote's (`requests::concern`).
            _ => {}
        }
    }

    /// The repository of the status `buffer`, if it may start a run now; else says why not,
    /// `no_upstream` when the run needs an upstream the branch lacks.
    fn remote_repo(&mut self, buffer: BufferId, no_upstream: Option<&str>) -> Option<usize> {
        let (project, root, built) = self.trusted_status(buffer)?;
        let index = self.git_repo(project, &root);
        if self.repo_busy(project) {
            self.editor.message(RUNNING);
            return None;
        }
        if built.branch.head.is_none() {
            self.editor
                .message("HEAD is detached: check out a branch first");
            return None;
        }
        if let Some(why) = no_upstream.filter(|_| built.branch.upstream.is_none()) {
            self.editor.message(why);
            return None;
        }
        Some(index)
    }

    fn push_git(&mut self, buffer: BufferId, target: PushTarget, switches: PushSwitches) {
        if target == PushTarget::Asked {
            return self.push_asked(buffer);
        }
        let Some(index) = self.remote_repo(buffer, None) else {
            return;
        };
        let pushes = &mut self.gits.commits.repos[index].pushes;
        pushes.asked = None;
        let prompted = pushes.prompt.take();
        let (want, switches) = match target {
            PushTarget::PushRemote => (Want::PushRemote, switches),
            PushTarget::Upstream => (Want::Upstream, switches),
            PushTarget::Elsewhere => (Want::Elsewhere, switches),
            PushTarget::Named(_) if prompted.as_ref().is_some_and(|p| p.status != buffer) => {
                return self
                    .editor
                    .message("That prompt was for another status's push: P again there");
            }
            PushTarget::Named(answer) => match prompted {
                Some(Prompted {
                    switches,
                    reads: Reads::Remote { set_upstream },
                    ..
                }) => (
                    Want::Remote {
                        remote: answer,
                        set_upstream,
                    },
                    switches,
                ),
                Some(Prompted { switches, .. }) => (Want::Branch(answer), switches),
                // Run by name, with no menu before it: a `/` names a branch.
                None if answer.contains('/') => (Want::Branch(answer), switches),
                None => (
                    Want::Remote {
                        remote: answer,
                        set_upstream: false,
                    },
                    switches,
                ),
            },
            PushTarget::Asked => return,
        };
        if self.ask_runner(
            index,
            RunRequest::Remote(RemoteRun::Plan { want, switches }),
        ) {
            self.gits.commits.repos[index].pushes.planning = Some(Planning {
                status: buffer,
                switches,
            });
        }
    }

    /// `y` to the question before a forced push: runs what it asked about.
    fn push_asked(&mut self, buffer: BufferId) {
        let Some(index) = self.remote_repo(buffer, None) else {
            return;
        };
        match self.gits.commits.repos[index].pushes.asked.take() {
            Some(asked) if asked.status == buffer => self.run_push(index, asked.push),
            _ => self
                .editor
                .message("The forced push asked about is gone: P -f asks again"),
        }
    }

    /// The run thread planned a push in the repository at `index`.
    pub(super) fn push_planned(&mut self, index: usize, planned: Result<Planned, String>) {
        let Some(wait) = self.gits.commits.repos[index].pushes.planning.take() else {
            return;
        };
        let (list, command, reads, items) = match planned {
            Err(why) => {
                // Echoed, not logged: git's words may quote the repository.
                return self
                    .editor
                    .message(format!("git: {}", one_line(&why, MAX_SAID)));
            }
            Ok(Planned::Ready(push)) if push.switches.force_with_lease => {
                let question = one_line(&push.question(), 400);
                match self.editor.ask_with(
                    format!("{question} (y or n)"),
                    "git-push-confirmed",
                    ["push", "keep"],
                ) {
                    Ok(prompt) => {
                        self.gits.commits.repos[index].pushes.asked = Some(Asked {
                            status: wait.status,
                            prompt,
                            push,
                        });
                    }
                    Err(e) => self.editor.message(format!("Cannot ask: {e}")),
                }
                return self.request_redraw();
            }
            Ok(Planned::Ready(push)) => return self.run_push(index, push),
            Ok(Planned::Remote {
                remotes,
                set_upstream,
            }) => (
                REMOTES_LIST,
                "git-push-to-remote",
                Reads::Remote { set_upstream },
                remotes,
            ),
            Ok(Planned::Branch { targets }) => {
                (TARGETS_LIST, "git-push-to", Reads::Branch, targets)
            }
        };
        let items: Arc<[Candidate]> = (items.iter())
            .map(|item| Candidate::new(item.as_str(), "", ""))
            .collect();
        self.editor.set_listed(list, items);
        let before = self.editor.prompt().map(oso_core::minibuffer::Prompt::id);
        let effects = self.dispatcher.run_from_host(&mut self.editor, command);
        // Only a prompt this opened takes the push's answer.
        let opened = self.editor.prompt().map(oso_core::minibuffer::Prompt::id);
        if let Some(prompt) = opened.filter(|id| Some(*id) != before) {
            self.gits.commits.repos[index].pushes.prompt = Some(Prompted {
                status: wait.status,
                prompt,
                switches: wait.switches,
                reads,
            });
        }
        if effects.redraw {
            self.request_redraw();
        }
    }

    /// After the requests: forgets each push's prompt or question that closed without its
    /// answer, by `n`, `C-g` or another prompt over it.
    pub(super) fn forget_closed_pushes(&mut self) {
        let editor = &self.editor;
        let open = |id: PromptId| editor.prompts().iter().any(|prompt| prompt.id() == id);
        for repo in &mut self.gits.commits.repos {
            let pushes = &mut repo.pushes;
            if pushes
                .prompt
                .as_ref()
                .is_some_and(|wait| !open(wait.prompt))
            {
                pushes.prompt = None;
            }
            if pushes
                .asked
                .as_ref()
                .is_some_and(|asked| !open(asked.prompt))
            {
                pushes.asked = None;
            }
        }
    }

    /// Starts `push`, planned, in the repository at `index`.
    fn run_push(&mut self, index: usize, push: Push) {
        let title = push.title();
        let done = format!("Pushed {} to {}", push.branch, push.destination());
        let pass = self.pass_env();
        if self.ask_runner(index, RunRequest::Remote(RemoteRun::Push { push, pass })) {
            self.remote_began(index, title, done);
        }
    }

    fn pull_git(&mut self, buffer: BufferId) {
        let no_upstream = "The branch has no upstream: P u sets one";
        let Some(index) = self.remote_repo(buffer, Some(no_upstream)) else {
            return;
        };
        let pass = self.pass_env();
        if self.ask_runner(index, RunRequest::Remote(RemoteRun::Pull { pass })) {
            self.remote_began(index, PULL.to_owned(), "Pulled, fast-forward".to_owned());
        }
    }

    fn fetch_git(&mut self, buffer: BufferId, from: FetchFrom) {
        let no_upstream = (from == FetchFrom::Upstream)
            .then_some("The branch has no upstream: f a fetches every remote");
        let Some(index) = self.remote_repo(buffer, no_upstream) else {
            return;
        };
        let pass = self.pass_env();
        let title = fetch_title(from);
        let done = match from {
            FetchFrom::Upstream => "Fetched from the upstream's remote",
            FetchFrom::All => "Fetched from every remote",
        };
        if self.ask_runner(index, RunRequest::Remote(RemoteRun::Fetch { from, pass })) {
            self.remote_began(index, title.to_owned(), done.to_owned());
        }
    }

    /// A remote's run called `title` began in the repository at `index`; `done` is what the
    /// echo area says once it went well.
    fn remote_began(&mut self, index: usize, title: String, done: String) {
        let echoed = format!("{title}…");
        self.editor.echo(echoed.clone());
        let log = self.git_log(index);
        self.editor.set_git_running(log, true);
        let repo = &mut self.gits.commits.repos[index];
        repo.run = Some(Running {
            kind: None,
            title,
            done,
            started: Instant::now(),
            echoed,
            progress: None,
            message: None,
            draft: None,
            output: String::new(),
        });
        self.request_redraw();
    }

    /// The names `[git] pass_env` passes on to a run's hooks.
    pub(super) fn pass_env(&self) -> Vec<String> {
        (self.configuration.git().pass_env.clone()).unwrap_or_default()
    }
}
