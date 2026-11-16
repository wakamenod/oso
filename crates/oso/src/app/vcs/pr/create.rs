//! Making and merging pull requests in the app (ADR 0127, step 5a-2): `N c`'s message buffer,
//! `*pull request: BRANCH*`, what `C-c C-c` makes of it, pushing the branch first after a
//! question, and `N m`'s merge after a question naming the pull request, its base, the method
//! and its head.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs `gh` or git: the
//! repository's `gh` thread reads what a message starts from and what a merge is of, and the
//! run thread makes and merges.
//!
//! - A message is the person's: an agent's command does not change it (`dispatch::agent`). Its
//!   text is kept for `M-p` when it is cancelled or its pull request is asked for.
//! - A question counts only while its prompt is open: one closed without its answer is
//!   forgotten, and its `y` runs only what it named.
//! - The pull request is made, or merged, as the person's run in the repository, with the log
//!   of git's runs, `C-g` stopping it; the status's `PR:` line is read again after.

use std::collections::VecDeque;

use oso_core::buffer::{Buffer, BufferId, Mode, Replacement};
use oso_core::editor::MergeMethod;
use oso_core::minibuffer::PromptId;
use oso_core::project::ProjectId;

use super::super::super::App;
use super::super::runs::RUNNING;
use crate::git::{TrustedRoot, printable};
use crate::vcs::pr::create::{PrRun, method_word, title_and_body};
use crate::vcs::pr::work::{Drafted, MergeTarget, PrRequest};
use crate::vcs::run::RunRequest;

/// The most messages kept for `M-p`.
const MAX_RING: usize = 20;
/// The longest message read back from its buffer.
const MAX_MESSAGE: usize = 256 * 1024;

/// What making and merging pull requests keeps.
#[derive(Default)]
pub(super) struct Creating {
    drafts: Vec<PrDraft>,
    ring: VecDeque<String>,
    /// The draft or merge being read on the `gh` thread: its generation, and what it was for.
    reading: Option<(u64, Reading)>,
    /// A question waiting for its `y`.
    asked: Option<(PromptId, Asked)>,
    /// A pull request to make once the push before it ends well, and its message's buffer.
    after_push: Option<(ProjectId, PrRun, BufferId)>,
    /// The project and draft switch of `N c - b`, while its prompt reads the base.
    base_asked: Option<(PromptId, ProjectId, bool)>,
}

enum Reading {
    Draft {
        draft: bool,
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
        buffer: BufferId,
        remote: Option<String>,
    },
    Merge {
        project: ProjectId,
        run: PrRun,
    },
}

/// A pull request's message being written.
struct PrDraft {
    buffer: BufferId,
    project: ProjectId,
    drafted: Drafted,
    draft: bool,
    ring_at: Option<usize>,
}

impl App {
    /// `N c`, or the answer to its base's prompt: an empty `base` opens that prompt first,
    /// keeping `draft` for its answer.
    pub(super) fn pr_create_asked(
        &mut self,
        project: ProjectId,
        draft: bool,
        base: Option<String>,
    ) {
        match base {
            Some(base) if base.is_empty() => {
                if let Some(prompt) = self.open_host_prompt("git-pr-create-base") {
                    self.gits.prs.create.base_asked = Some((prompt, project, draft));
                }
            }
            Some(base) => {
                // The prompt's answer carries no switch: the draft is the one `N c` had.
                let asked = self.gits.prs.create.base_asked.take();
                let draft = asked.is_some_and(|(_, of, draft)| of == project && draft);
                self.start_pr(project, draft, Some(base));
            }
            None => self.start_pr(project, draft, None),
        }
    }

    /// Reads what the branch's pull request starts from, into `base` or the default.
    fn start_pr(&mut self, project: ProjectId, draft: bool, base: Option<String>) {
        self.gits.prs.generation += 1;
        let generation = self.gits.prs.generation;
        if self
            .ask_pr(project, PrRequest::Draft { generation, base })
            .is_some()
        {
            self.gits.prs.create.reading = Some((generation, Reading::Draft { draft }));
            self.editor.echo("Reading the branch's commits…");
        }
    }

    /// `N m`: reads the branch's open pull request, to ask about merging it.
    pub(super) fn start_merge(&mut self, project: ProjectId, method: MergeMethod, delete: bool) {
        self.gits.prs.generation += 1;
        let generation = self.gits.prs.generation;
        if self
            .ask_pr(project, PrRequest::MergeAsk { generation })
            .is_some()
        {
            let reading = Reading::Merge {
                method,
                delete_branch: delete,
            };
            self.gits.prs.create.reading = Some((generation, reading));
        }
    }

    /// What a pull request's message starts from came.
    pub(super) fn pr_drafted(
        &mut self,
        project: ProjectId,
        generation: u64,
        drafted: Result<Drafted, String>,
    ) {
        let Some((asked, Reading::Draft { draft })) = self.gits.prs.create.reading.take() else {
            return;
        };
        if asked != generation {
            return;
        }
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
        let name = self.unique_name(&format!("*pull request: {}*", drafted.branch));
        let mut buffer = Buffer::from_text(name, &drafted.text);
        buffer.set_mode(Mode::GitPrMessage);
        let id = self.editor.add_buffer(buffer);
        self.stand_in_root(id, project);
        self.editor.show(id);
        let into = format!("{} into {}", drafted.branch, drafted.base);
        self.gits.prs.create.drafts.push(PrDraft {
            buffer: id,
            project,
            drafted,
            draft,
            ring_at: None,
        });
        self.editor.echo(format!(
            "{into}: the first line is the title. C-c C-c makes it, C-c C-k cancels, M-p brings \
             back an earlier message"
        ));
        self.request_redraw();
    }

    /// `C-c C-c` in a message: makes its pull request, after asking to push the branch first if
    /// it has no upstream or is ahead of it.
    pub(super) fn finish_pr(&mut self, buffer: BufferId) {
        let Some(at) = self.pr_draft(buffer) else {
            return;
        };
        let Some(text) = (self.editor.buffer(buffer))
            .filter(|shown| shown.text().len() <= MAX_MESSAGE)
            .map(|shown| shown.text().to_string())
        else {
            return self
                .editor
                .message("The message is longer than oso gives gh");
        };
        let (title, body) = match title_and_body(&text) {
            Ok(both) => both,
            Err(why) => return self.editor.message(why),
        };
        let draft = &self.gits.prs.create.drafts[at];
        let (project, drafted) = (draft.project, &draft.drafted);
        let run = PrRun::Create {
            repo: drafted.repo.clone(),
            base: drafted.base.clone(),
            head: drafted.branch.clone(),
            title,
            body,
            draft: draft.draft,
        };
        if !drafted.push {
            return self.run_pr(project, run, Some(buffer));
        }
        let has_upstream = (self.gits.open.iter())
            .find(|open| open.project == project)
            .is_some_and(|open| open.built.branch.upstream.is_some());
        let remote = (!has_upstream).then(|| drafted.remote.clone());
        let question = format!(
            "Push {} to {} first? A pull request is of what the remote has",
            drafted.branch, drafted.remote
        );
        if let Some(prompt) = self.ask_bound(&question, "git-pr-confirmed", ["yes", "keep"]) {
            let asked = Asked::PushFirst {
                project,
                run,
                buffer,
                remote,
            };
            self.gits.prs.create.asked = Some((prompt, asked));
        }
    }

    /// What a merge is of came: asks about it, naming the pull request, its base, the method and
    /// its head.
    pub(super) fn pr_merge_read(
        &mut self,
        project: ProjectId,
        generation: u64,
        target: Result<MergeTarget, String>,
    ) {
        let Some((
            asked,
            Reading::Merge {
                method,
                delete_branch,
            },
        )) = self.gits.prs.create.reading.take()
        else {
            return;
        };
        if asked != generation {
            return;
        }
        let target = match target {
            Ok(target) => target,
            Err(why) => {
                return self
                    .editor
                    .message(printable(&why, super::super::MAX_MESSAGE));
            }
        };
        let short = target.head.get(..7).unwrap_or(&target.head);
        let deleting = if delete_branch {
            ", deleting its branch on the remote"
        } else {
            ""
        };
        let question = format!(
            "Merge #{} into {} by {}, its head {short}{deleting}?",
            target.number,
            printable(&target.base, 80),
            method_word(method)
        );
        let run = PrRun::Merge {
            repo: target.repo,
            number: target.number,
            method,
            head: target.head,
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
                buffer,
                remote,
            } => {
                let Some(status) = self.editor.git_status_buffer(project) else {
                    return self
                        .editor
                        .message("The branch is pushed from its status: C-x g, then P");
                };
                if self.push_for_pr(status, remote) {
                    self.gits.prs.create.after_push = Some((project, run, buffer));
                }
            }
        }
    }

    /// After a run in `project`'s repository ended: makes the pull request waiting for its push,
    /// if the push went well.
    pub(in crate::app::vcs) fn pr_after_push(&mut self, project: ProjectId, pushed: bool) {
        let waiting = self.gits.prs.create.after_push.take();
        let Some((of, run, buffer)) = waiting else {
            return;
        };
        if of != project {
            self.gits.prs.create.after_push = Some((of, run, buffer));
            return;
        }
        if pushed {
            self.run_pr(project, run, Some(buffer));
        } else {
            self.editor
                .message("The push did not go: the pull request is not made; C-c C-c again");
        }
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
            self.keep_pr_message(buffer);
            self.drop_pr_draft(buffer);
        }
    }

    /// `C-c C-k`: drops the message, keeping its text for `M-p`.
    pub(super) fn cancel_pr(&mut self, buffer: BufferId) {
        if self.pr_draft(buffer).is_none() {
            return;
        }
        self.keep_pr_message(buffer);
        self.drop_pr_draft(buffer);
        self.editor
            .message("Pull request cancelled; M-p in the next message brings it back");
        self.request_redraw();
    }

    /// `M-p`, `M-n`: an earlier message in `buffer`, or a later one.
    pub(super) fn walk_pr_ring(&mut self, buffer: BufferId, older: bool) {
        let Some(at) = self.pr_draft(buffer) else {
            return;
        };
        let creating = &self.gits.prs.create;
        let next = match (creating.drafts[at].ring_at, older) {
            (None, true) => Some(0),
            (Some(n), true) => Some(n + 1),
            (Some(n), false) => n.checked_sub(1),
            (None, false) => None,
        };
        let Some((next, kept)) = next.and_then(|n| Some((n, creating.ring.get(n)?.clone()))) else {
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
        self.gits.prs.create.drafts[at].ring_at = Some(next);
        self.editor
            .message(format!("Message {} of {count} kept", next + 1));
    }

    /// After each event: forgets messages whose buffers were killed, and a question whose
    /// prompt closed without its answer.
    pub(super) fn sync_pr_drafts(&mut self) {
        let editor = &self.editor;
        (self.gits.prs.create.drafts).retain(|draft| editor.buffer(draft.buffer).is_some());
        let open = |prompt: PromptId| editor.prompt().is_some_and(|shown| shown.id() == prompt);
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

    fn pr_draft(&self, buffer: BufferId) -> Option<usize> {
        (self.gits.prs.create.drafts.iter()).position(|draft| draft.buffer == buffer)
    }

    fn keep_pr_message(&mut self, buffer: BufferId) {
        let text = (self.editor.buffer(buffer))
            .filter(|shown| shown.text().len() <= MAX_MESSAGE)
            .map(|shown| shown.text().to_string());
        let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
            return;
        };
        let ring = &mut self.gits.prs.create.ring;
        ring.retain(|kept| *kept != text);
        ring.push_front(text);
        ring.truncate(MAX_RING);
    }

    fn drop_pr_draft(&mut self, buffer: BufferId) {
        let project = (self.pr_draft(buffer)).map(|at| self.gits.prs.create.drafts[at].project);
        (self.gits.prs.create.drafts).retain(|draft| draft.buffer != buffer);
        if let Some(status) = project.and_then(|project| self.editor.git_status_buffer(project)) {
            let showing: Vec<_> = (self.editor.windows())
                .filter(|window| window.buffer() == buffer)
                .map(|window| window.id())
                .collect();
            for window in showing {
                self.editor.show_in(window, status);
            }
        }
        self.editor.kill_buffer(buffer);
    }
}
