//! Making and merging pull requests in the app (ADR 0127, step 5a-2): `N c`'s message buffer,
//! `*pull request: BRANCH*`, what `C-c C-c` makes of it, pushing the branch first after a
//! question, and `N m`'s merge after a question naming the pull request, its repository, its
//! base, the method and its head.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs `gh` or git: the
//! repository's `gh` thread reads what a message starts from and what a merge is of, and the
//! run thread makes and merges.
//!
//! - A message is the person's: an agent's command does not change it (`dispatch::agent`). Its
//!   text is kept for `M-p` when it is cancelled or its pull request is asked for, unless a
//!   secret was yanked into it.
//! - What `gh` reads late opens nothing over what the person does now: a message or a question
//!   comes only while they are still where they asked, else they are told to ask again.
//! - A question counts only while its prompt is open, and its `y` runs only what it named. The
//!   push before a pull request goes to the remote's branch of the same name, and the pull
//!   request is made only after that push, of that branch, went well, while its message is open.
//! - The pull request is made, or merged, as the person's run in the repository, with the log
//!   of git's runs, `C-g` stopping it; the status's `PR:` line is read again after.

use oso_core::buffer::{Buffer, BufferId, Mode, Replacement};
use oso_core::editor::MergeMethod;
use oso_core::minibuffer::PromptId;
use oso_core::project::ProjectId;

use super::super::super::App;
use super::super::commit::{cancelled_says, ring_text};
use super::super::runs::RUNNING;
use crate::commands::vcs::pr::create::SECRET_MESSAGE;
use crate::git::{TrustedRoot, printable};
use crate::vcs::commit::Ring;
use crate::vcs::pr::create::{MAX_PR_MESSAGE, PrRun, method_word, title_and_body};
use crate::vcs::pr::work::{Drafted, MergeTarget, PrRequest};
use crate::vcs::run::RunRequest;

/// What making and merging pull requests keeps.
#[derive(Default)]
pub(super) struct Creating {
    messages: Vec<PrMessage>,
    ring: Ring,
    /// The draft or merge being read on the `gh` thread: its generation, the buffer it was
    /// asked from, and what it was for.
    reading: Option<(u64, BufferId, Reading)>,
    /// A question waiting for its `y`.
    asked: Option<(PromptId, Asked)>,
    /// The project and draft switch of `N c - b`, while its prompt reads the base.
    base_asked: Option<(PromptId, ProjectId, bool)>,
}

enum Reading {
    Draft {
        as_draft: bool,
    },
    Merge {
        method: MergeMethod,
        delete_branch: bool,
    },
}

enum Asked {
    PushFirst {
        project: ProjectId,
        run: PrRun,
        message: BufferId,
        branch: String,
        to: (String, bool),
    },
    Merge {
        project: ProjectId,
        run: PrRun,
    },
}

/// What follows a run once it ended (step 5a-2): the pull request whose branch it pushed.
pub(in crate::app::vcs) enum AfterRun {
    CreatePr { run: PrRun, message: BufferId },
}

/// A pull request's message being written.
struct PrMessage {
    buffer: BufferId,
    project: ProjectId,
    drafted: Drafted,
    as_draft: bool,
    ring_at: Option<usize>,
}

impl App {
    /// `N c`: reads what the branch's pull request starts from, or with `- b` first opens the
    /// base's prompt, keeping `as_draft` for its answer.
    pub(super) fn pr_create_asked(&mut self, project: ProjectId, as_draft: bool, ask_base: bool) {
        if !ask_base {
            return self.start_pr(project, as_draft, None);
        }
        if let Some(prompt) = self.open_host_prompt("git-pr-create-base") {
            self.gits.prs.create.base_asked = Some((prompt, project, as_draft));
        }
    }

    /// What `N c - b`'s prompt read: the base, with the draft switch `N c` had.
    pub(super) fn pr_base_answered(&mut self, project: ProjectId, base: String) {
        let Some((_, of, as_draft)) = self.gits.prs.create.base_asked.take() else {
            return self
                .editor
                .message("The base's prompt is gone: N c - b asks again");
        };
        if of != project {
            return self
                .editor
                .message("That base was for another project's pull request");
        }
        self.start_pr(project, as_draft, Some(base));
    }

    /// Reads what the branch's pull request starts from, into `base` or the default.
    fn start_pr(&mut self, project: ProjectId, as_draft: bool, base: Option<String>) {
        self.gits.prs.generation += 1;
        let generation = self.gits.prs.generation;
        let from = self.editor.current().0.buffer();
        if self
            .ask_pr(project, PrRequest::Draft { generation, base })
            .is_some()
        {
            let reading = Reading::Draft { as_draft };
            self.gits.prs.create.reading = Some((generation, from, reading));
            self.editor.echo("Reading the branch's commits…");
        }
    }

    /// `N m`: reads the branch's open pull request, to ask about merging it.
    pub(super) fn start_merge(&mut self, project: ProjectId, method: MergeMethod, delete: bool) {
        self.gits.prs.generation += 1;
        let generation = self.gits.prs.generation;
        let from = self.editor.current().0.buffer();
        if self
            .ask_pr(project, PrRequest::MergeAsk { generation })
            .is_some()
        {
            let reading = Reading::Merge {
                method,
                delete_branch: delete,
            };
            self.gits.prs.create.reading = Some((generation, from, reading));
        }
    }

    /// Takes the reading `generation` is the answer to, if it is the one waited for and of the
    /// kind `draft` says, with the buffer it was asked from; else leaves it.
    fn answered(&mut self, generation: u64, draft: bool) -> Option<(BufferId, Reading)> {
        let reading = &self.gits.prs.create.reading;
        let ours = reading.as_ref().is_some_and(|(asked, _, kind)| {
            *asked == generation && matches!(kind, Reading::Draft { .. }) == draft
        });
        if !ours {
            return None;
        }
        let (_, from, kind) = self.gits.prs.create.reading.take()?;
        Some((from, kind))
    }

    /// What a pull request's message starts from came: its buffer opens, if the person is still
    /// where they asked.
    pub(super) fn pr_drafted(
        &mut self,
        project: ProjectId,
        generation: u64,
        drafted: Result<Drafted, String>,
    ) {
        let Some((from, Reading::Draft { as_draft })) = self.answered(generation, true) else {
            return;
        };
        self.editor
            .clear_echo_saying("Reading the branch's commits…");
        let drafted = match drafted {
            Ok(drafted) => drafted,
            Err(why) => {
                return self
                    .editor
                    .message(printable(&why, super::super::MAX_MESSAGE));
            }
        };
        if !self.still_at(from) {
            return self
                .editor
                .message("The pull request's message is read: N c asks again");
        }
        let branch = printable(&drafted.branch, 80);
        let name = self.unique_name(&format!("*pull request: {branch}*"));
        let mut buffer = Buffer::from_text(name, &drafted.text);
        buffer.set_mode(Mode::GitPrMessage);
        let id = self.editor.add_buffer(buffer);
        self.stand_in_root(id, project);
        self.editor.show(id);
        let into = format!("{branch} into {}", printable(&drafted.base, 80));
        self.gits.prs.create.messages.push(PrMessage {
            buffer: id,
            project,
            drafted,
            as_draft,
            ring_at: None,
        });
        self.editor.echo(format!(
            "{into}: the first line is the title. C-c C-c makes it, C-c C-k cancels, M-p brings \
             back an earlier message"
        ));
        self.request_redraw();
    }

    /// `C-c C-c` in a message: makes its pull request, after asking to push the branch first if
    /// the remote's branch of its name lacks it.
    pub(super) fn finish_pr(&mut self, buffer: BufferId) {
        let Some(at) = self.pr_message(buffer) else {
            return;
        };
        // Also here, where its text is taken: a yank may have turned it secret since.
        if self.editor.buffer(buffer).is_some_and(Buffer::is_secret) {
            return self.editor.message(SECRET_MESSAGE);
        }
        let Some(text) = self.pr_message_text(buffer) else {
            return self
                .editor
                .message("The message is longer than oso gives gh");
        };
        let (title, body) = match title_and_body(&text) {
            Ok(both) => both,
            Err(why) => return self.editor.message(why),
        };
        let message = &self.gits.prs.create.messages[at];
        let (project, drafted) = (message.project, &message.drafted);
        let run = PrRun::Create {
            repo: drafted.repo.clone(),
            base: drafted.base.clone(),
            head: drafted.branch.clone(),
            title,
            body,
            draft: message.as_draft,
        };
        if !drafted.push_first {
            return self.run_pr(project, run, Some(buffer));
        }
        let (branch, remote) = (drafted.branch.clone(), drafted.remote.clone());
        let question = format!(
            "Push {b} to {r}/{b} first? A pull request is of what the remote has",
            b = printable(&branch, 80),
            r = printable(&remote, 40),
        );
        let to = (remote, drafted.set_upstream);
        if let Some(prompt) = self.ask_bound(&question, "git-pr-confirmed", ["yes", "keep"]) {
            let asked = Asked::PushFirst {
                project,
                run,
                message: buffer,
                branch,
                to,
            };
            self.gits.prs.create.asked = Some((prompt, asked));
        }
    }

    /// What a merge is of came: asks about it, naming the pull request, its repository, its
    /// base, the method and its head, if the person is still where they asked.
    pub(super) fn pr_merge_read(
        &mut self,
        project: ProjectId,
        generation: u64,
        target: Result<MergeTarget, String>,
    ) {
        let Some((
            from,
            Reading::Merge {
                method,
                delete_branch,
            },
        )) = self.answered(generation, false)
        else {
            return;
        };
        let target = match target {
            Ok(target) => target,
            Err(why) => {
                return self
                    .editor
                    .message(printable(&why, super::super::MAX_MESSAGE));
            }
        };
        if !self.still_at(from) {
            return self
                .editor
                .message("The pull request to merge is read: N m asks again");
        }
        let short = target.head_commit.get(..7).unwrap_or(&target.head_commit);
        let deleting = if delete_branch {
            ", deleting its branch on the remote"
        } else {
            ""
        };
        let question = format!(
            "Merge #{} of {} into {} by {}, its head {short}{deleting}?",
            target.number,
            printable(&target.repo, 120),
            printable(&target.base, 80),
            method_word(method)
        );
        let run = PrRun::Merge {
            repo: target.repo,
            number: target.number,
            method,
            base: target.base,
            head_commit: target.head_commit,
            delete_branch,
        };
        if let Some(prompt) = self.ask_bound(&question, "git-pr-confirmed", ["yes", "keep"]) {
            self.gits.prs.create.asked = Some((prompt, Asked::Merge { project, run }));
        }
    }

    /// The `y` to a pull request's question.
    pub(super) fn pr_confirmed(&mut self) {
        let Some((_, asked)) = self.gits.prs.create.asked.take() else {
            return self.editor.message("The question is gone: ask again");
        };
        match asked {
            Asked::Merge { project, run } => self.run_pr(project, run, None),
            Asked::PushFirst {
                project,
                run,
                message,
                branch,
                to,
            } => {
                let Some(status) = self.editor.git_status_buffer(project) else {
                    return self
                        .editor
                        .message("The branch is pushed from its status: C-x g, then P");
                };
                let then = AfterRun::CreatePr { run, message };
                self.push_for_pr(status, to, branch, then);
            }
        }
    }

    /// After a run in `project`'s repository that `then` was to follow ended, `done` if it went
    /// well: makes the pull request whose push it was, while its message is still open.
    pub(in crate::app::vcs) fn after_run(
        &mut self,
        project: ProjectId,
        then: AfterRun,
        done: bool,
    ) {
        let AfterRun::CreatePr { run, message } = then;
        if !done {
            return self
                .editor
                .message("The push did not go: the pull request is not made; C-c C-c again");
        }
        if self.pr_message(message).is_none() {
            return self
                .editor
                .message("The pull request's message was closed: it is not made");
        }
        self.run_pr(project, run, Some(message));
    }

    /// Starts `run` in `project`'s repository; a message's buffer goes once its run began, its
    /// text kept for `M-p`.
    fn run_pr(&mut self, project: ProjectId, run: PrRun, message: Option<BufferId>) {
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(super::RESTRICTED);
        };
        let repo = self.git_repo(project, &root);
        if self.repo_busy(project) {
            return self.editor.message(RUNNING);
        }
        let (title, done) = (run.title(), run.done());
        if !self.ask_runner(repo, RunRequest::Pr { run }) {
            return;
        }
        self.run_began(repo, title, done);
        if let Some(buffer) = message {
            self.drop_pr_message(buffer);
        }
    }

    /// `C-c C-k`: drops the message, keeping its text for `M-p`.
    pub(super) fn cancel_pr(&mut self, buffer: BufferId) {
        if self.pr_message(buffer).is_none() {
            return;
        }
        let says = cancelled_says("Pull request cancelled", self.editor.buffer(buffer));
        self.drop_pr_message(buffer);
        self.editor.message(says);
        self.request_redraw();
    }

    /// `M-p`, `M-n`: an earlier message in `buffer`, or a later one.
    pub(super) fn walk_pr_ring(&mut self, buffer: BufferId, older: bool) {
        let Some(at) = self.pr_message(buffer) else {
            return;
        };
        let creating = &self.gits.prs.create;
        let next = creating.ring.step(creating.messages[at].ring_at, older);
        let Some((next, kept)) = next.and_then(|n| Some((n, creating.ring.get(n)?.to_owned())))
        else {
            let which = if older { "older" } else { "newer" };
            return self.editor.message(format!("No {which} message kept"));
        };
        let len = self
            .editor
            .buffer(buffer)
            .map_or(0, |shown| shown.text().len());
        let edit = Replacement {
            range: 0..len,
            text: &kept,
        };
        if let Err(e) = self.editor.apply_edits(buffer, &[edit]) {
            return self.editor.message(format!("Cannot put it in: {e}"));
        }
        let count = self.gits.prs.create.ring.len();
        self.gits.prs.create.messages[at].ring_at = Some(next);
        self.editor
            .message(format!("Message {} of {count} kept", next + 1));
    }

    /// After each event: forgets messages whose buffers were killed, and a question or a prompt
    /// that closed without its answer.
    pub(super) fn sync_pr_drafts(&mut self) {
        let editor = &self.editor;
        (self.gits.prs.create.messages).retain(|message| editor.buffer(message.buffer).is_some());
        let open = |id: PromptId| editor.prompts().iter().any(|prompt| prompt.id() == id);
        let create = &mut self.gits.prs.create;
        if create
            .asked
            .as_ref()
            .is_some_and(|(prompt, _)| !open(*prompt))
        {
            create.asked = None;
        }
        if create
            .base_asked
            .as_ref()
            .is_some_and(|(prompt, ..)| !open(*prompt))
        {
            create.base_asked = None;
        }
    }

    fn pr_message(&self, buffer: BufferId) -> Option<usize> {
        (self.gits.prs.create.messages.iter()).position(|message| message.buffer == buffer)
    }

    /// The text of `buffer`, unless it is gone or longer than a pull request's message: its
    /// length is looked at before anything is copied.
    fn pr_message_text(&self, buffer: BufferId) -> Option<String> {
        let shown = self.editor.buffer(buffer)?;
        (shown.text().len() <= MAX_PR_MESSAGE).then(|| shown.text().to_string())
    }

    /// Keeps the message's text for `M-p`, unless a secret was yanked into it, then kills its
    /// buffer.
    fn drop_pr_message(&mut self, buffer: BufferId) {
        let shown = self.editor.buffer(buffer);
        if let Some(text) = shown.and_then(|shown| ring_text(shown, MAX_PR_MESSAGE)) {
            self.gits.prs.create.ring.keep(text);
        }
        let project = (self.pr_message(buffer)).map(|at| self.gits.prs.create.messages[at].project);
        (self.gits.prs.create.messages).retain(|message| message.buffer != buffer);
        self.close_message(buffer, project);
    }
}
