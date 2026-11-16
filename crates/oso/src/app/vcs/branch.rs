//! Branches in the app (ADR 0127, step 3b): the branch menu's heads, the prompts each opens in
//! turn (a branch, where to make one, a name), the question before a forced delete, and
//! starting each run on the repository's run thread.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or waits for
//! it: the repository's run thread lists the branches, plans a delete and runs.
//!
//! - Each head lists the branches on the run thread first, then opens its prompt as its keys
//!   would, but only while the person is still in that status with no prompt open; an answer
//!   counts only for the status and the prompt it was asked over, and one closed without it is
//!   forgotten, as a push's is.
//! - A prompt keeps what it listed, so that an answer names the ref the person saw: a remote's
//!   branch, or a start, goes to git in full (`refs/remotes/…`), and no tag of the same short
//!   name stands in for it.
//! - Checking out, and making a branch to check out, are refused while a merge, rebase, pick,
//!   revert or `git am` is stopped (Q6), checked again as each runs.
//! - A delete that is not merged asks first, naming the commits only that branch holds; its
//!   `y` runs `-D` only while the branch is where the question saw it.

use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::complete::Candidate;
use oso_core::editor::{BranchAction, GitRequest};
use oso_core::minibuffer::PromptId;

use super::super::App;
use super::runs::{MAX_SAID, RUNNING};
use crate::commands::review::one_line;
use crate::commands::vcs::branch::{BRANCHES_LIST, STARTS_LIST};
use crate::vcs::branch::{self, BranchRead, BranchRun, Branches, DeletePlan};
use crate::vcs::layout::Built;
use crate::vcs::run::RunRequest;

/// The most branches a prompt lists, as a push's: the most a completion opens with on the UI
/// thread before its list is built on the worker.
const LISTED: usize = 5000;

/// What a head does with the branch it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BranchHead {
    Checkout,
    Create { checkout: bool },
    Rename,
    Delete,
}

/// A name a prompt listed, and the ref git is given for it.
#[derive(Clone, Debug)]
struct Shown {
    name: String,
    full: String,
    remote: bool,
}

/// What a branch's prompt reads.
#[derive(Clone, Debug)]
enum Reads {
    /// A branch to check out, rename or delete, of those `shown`.
    Pick(BranchHead, Vec<Shown>),
    /// Where to make a branch: one of those `shown`, or any revision typed.
    Start { checkout: bool, shown: Vec<Shown> },
    /// The new branch's name, made at `start`.
    Name { checkout: bool, start: String },
    /// The name `old` is renamed to.
    NewName { old: String },
}

/// A prompt waiting for its answer, opened over the status `status`.
struct Prompted {
    status: BufferId,
    prompt: PromptId,
    reads: Reads,
}

/// A forced delete waiting for the `y` to the question `prompt`, asked over `status`.
struct Asked {
    status: BufferId,
    prompt: PromptId,
    name: String,
    tip: String,
}

/// A repository's branch heads waiting: a list or a delete's plan from the run thread, a
/// prompt, a question.
#[derive(Default)]
pub(super) struct Branching {
    listing: Option<(BufferId, BranchHead)>,
    deleting: Option<BufferId>,
    prompt: Option<Prompted>,
    asked: Option<Asked>,
}

impl Branching {
    /// Whether the run thread has a list or a plan to give back.
    pub(super) fn planning(&self) -> bool {
        self.listing.is_some() || self.deleting.is_some()
    }
}

impl App {
    /// Carries out what the branch menu's heads, and the prompts and question they open, asked.
    pub(super) fn git_branch_request(&mut self, request: GitRequest) {
        let GitRequest::Branch { buffer, action } = request else {
            // The others are not a branch's (`requests::concern`).
            return;
        };
        let Some((project, root, built)) = self.trusted_status(buffer) else {
            return;
        };
        let index = self.git_repo(project, &root);
        if self.repo_busy(project) {
            return self.editor.message(RUNNING);
        }
        let waits = &mut self.gits.commits.repos[index].branches;
        let prompted = waits.prompt.take().filter(|wait| wait.status == buffer);
        let asked = waits.asked.take().filter(|wait| wait.status == buffer);
        let head = match action {
            BranchAction::Checkout => BranchHead::Checkout,
            BranchAction::Create { checkout } => BranchHead::Create { checkout },
            BranchAction::Rename => BranchHead::Rename,
            BranchAction::Delete => BranchHead::Delete,
            BranchAction::Picked(text) | BranchAction::Named(text) => {
                return match prompted {
                    Some(wait) => self.branch_answered(index, (&built, buffer), wait.reads, &text),
                    None => self
                        .editor
                        .message("The branch's prompt is gone: b asks again"),
                };
            }
            BranchAction::Confirmed => {
                return match asked {
                    Some(Asked { name, tip, .. }) => {
                        let run = BranchRun::Delete {
                            name,
                            tip: Some(tip),
                        };
                        self.run_branch(index, &built, run);
                    }
                    None => self
                        .editor
                        .message("The delete asked about is gone: b k asks again"),
                };
            }
        };
        let checks_out = matches!(
            head,
            BranchHead::Checkout | BranchHead::Create { checkout: true }
        );
        if checks_out && self.refused_while_stopped(&built) {
            return;
        }
        if self.ask_runner(index, RunRequest::BranchRead(BranchRead::List)) {
            self.gits.commits.repos[index].branches.listing = Some((buffer, head));
        }
    }

    /// The run thread listed the branches of the repository at `index`: opens the prompt the
    /// head waiting asked for, if the person is still where they asked.
    pub(super) fn branches_listed(&mut self, index: usize, read: Result<Branches, String>) {
        let Some((status, head)) = self.gits.commits.repos[index].branches.listing.take() else {
            return;
        };
        let branches = match read {
            Ok(branches) => branches,
            Err(why) => {
                return self
                    .editor
                    .message(format!("git: {}", one_line(&why, MAX_SAID)));
            }
        };
        if !self.still_at(status) {
            return self.editor.message("The branches are read: b asks again");
        }
        let (list, command, reads) = reads_for(head, &branches);
        let shown = match &reads {
            Reads::Pick(_, shown) | Reads::Start { shown, .. } => shown.as_slice(),
            Reads::Name { .. } | Reads::NewName { .. } => &[],
        };
        if shown.is_empty() {
            return self.editor.message("There is no other branch here");
        }
        let current = branches.current.as_deref();
        let items: Arc<[Candidate]> = (shown.iter())
            .map(|shown| {
                let note = if shown.remote {
                    "remote"
                } else if Some(shown.name.as_str()) == current {
                    "checked out"
                } else if shown.full == "HEAD" {
                    "where you are"
                } else {
                    ""
                };
                Candidate::new(shown.name.as_str(), "", note)
            })
            .collect();
        self.editor.set_listed(list, items);
        self.open_branch_prompt(index, status, command, reads);
    }

    /// What the prompt `reads` read as `text`, over the status `status`.
    fn branch_answered(
        &mut self,
        index: usize,
        (built, status): (&Built, BufferId),
        reads: Reads,
        text: &str,
    ) {
        let text = text.trim();
        if text.is_empty() {
            return self.editor.message("No branch was named");
        }
        let listed = |shown: &[Shown]| shown.iter().find(|shown| shown.name == text).cloned();
        match reads {
            Reads::Pick(head, shown) => {
                let Some(picked) = listed(&shown) else {
                    return self
                        .editor
                        .message(format!("{} is not one of those listed", one_line(text, 80)));
                };
                self.branch_picked(index, (built, status), head, picked);
            }
            Reads::Start { checkout, shown } => {
                // A start listed goes in full; one typed, as typed.
                let start = listed(&shown).map_or_else(|| text.to_owned(), |shown| shown.full);
                let reads = Reads::Name { checkout, start };
                self.open_branch_prompt(index, status, "git-branch-name", reads);
            }
            Reads::Name { checkout, start } => {
                let run = BranchRun::Create {
                    name: text.to_owned(),
                    start,
                    checkout,
                };
                self.run_branch(index, built, run);
            }
            Reads::NewName { old } => {
                let run = BranchRun::Rename {
                    old,
                    new: text.to_owned(),
                };
                self.run_branch(index, built, run);
            }
        }
    }

    /// `head` with the branch `picked` from its prompt.
    fn branch_picked(
        &mut self,
        index: usize,
        (built, status): (&Built, BufferId),
        head: BranchHead,
        picked: Shown,
    ) {
        match head {
            BranchHead::Checkout => {
                let run = BranchRun::Checkout {
                    name: picked.name,
                    remote: picked.remote,
                };
                self.run_branch(index, built, run);
            }
            BranchHead::Rename => {
                let reads = Reads::NewName { old: picked.name };
                self.open_branch_prompt(index, status, "git-branch-new-name", reads);
            }
            BranchHead::Delete => {
                let read = BranchRead::PlanDelete { name: picked.name };
                if self.ask_runner(index, RunRequest::BranchRead(read)) {
                    self.gits.commits.repos[index].branches.deleting = Some(status);
                }
            }
            // A start is read by its own prompt.
            BranchHead::Create { .. } => {}
        }
    }

    /// The run thread planned deleting `name` in the repository at `index`.
    pub(super) fn delete_planned(
        &mut self,
        index: usize,
        name: String,
        plan: Result<DeletePlan, String>,
    ) {
        let Some(status) = self.gits.commits.repos[index].branches.deleting.take() else {
            return;
        };
        let (tip, dropped) = match plan {
            Err(why) => return self.editor.message(one_line(&why, MAX_SAID)),
            Ok(DeletePlan::Merged) => {
                let run = BranchRun::Delete { name, tip: None };
                return self.run_branch(index, &Built::default(), run);
            }
            Ok(DeletePlan::Unmerged { tip, dropped }) => (tip, dropped),
        };
        if !self.still_at(status) {
            return self
                .editor
                .message("The delete was planned: b k asks again");
        }
        let question = branch::delete_question(&name, dropped.as_ref());
        let yes = "git-branch-delete-confirmed";
        if let Some(prompt) = self.ask_bound(&question, yes, ["delete", "keep"]) {
            self.gits.commits.repos[index].branches.asked = Some(Asked {
                status,
                prompt,
                name,
                tip,
            });
        }
    }

    /// Opens `command`'s prompt over the status `status`, and keeps it for its answer.
    fn open_branch_prompt(&mut self, index: usize, status: BufferId, command: &str, reads: Reads) {
        if let Some(prompt) = self.open_host_prompt(command) {
            self.gits.commits.repos[index].branches.prompt = Some(Prompted {
                status,
                prompt,
                reads,
            });
        }
    }

    /// Starts `run` in the repository at `index`; one that checks out is refused while `built`
    /// shows something stopped.
    fn run_branch(&mut self, index: usize, built: &Built, run: BranchRun) {
        let checks_out = matches!(
            run,
            BranchRun::Checkout { .. } | BranchRun::Create { checkout: true, .. }
        );
        if checks_out && self.refused_while_stopped(built) {
            return;
        }
        let (title, done) = (run.title(), run.done());
        let pass = self.pass_env();
        if self.ask_runner(index, RunRequest::Branch { run, pass }) {
            self.run_began(index, title, done);
        }
    }

    /// After the requests: forgets each branch's prompt or question that closed without its
    /// answer.
    pub(super) fn forget_closed_branch_prompts(&mut self) {
        let editor = &self.editor;
        let open = |id: PromptId| editor.prompts().iter().any(|prompt| prompt.id() == id);
        for repo in &mut self.gits.commits.repos {
            let waits = &mut repo.branches;
            waits.prompt.take_if(|wait| !open(wait.prompt));
            waits.asked.take_if(|asked| !open(asked.prompt));
        }
    }
}

/// The list, the prompt's command and what it reads for `head` over `branches`: the local
/// branches before the remotes', `HEAD` first where a branch is made, the one checked out first
/// to rename, and left out to check out or delete; at most [`LISTED`].
fn reads_for(head: BranchHead, branches: &Branches) -> (&'static str, &'static str, Reads) {
    let current = branches.current.as_deref();
    let local = |name: &String| Shown {
        name: name.clone(),
        full: format!("refs/heads/{name}"),
        remote: false,
    };
    let remote = |name: &String| Shown {
        name: name.clone(),
        full: format!("refs/remotes/{name}"),
        remote: true,
    };
    let others = || (branches.local.iter()).filter(|name| Some(name.as_str()) != current);
    let pick = |shown: Vec<Shown>| (BRANCHES_LIST, "git-branch-pick", Reads::Pick(head, shown));
    match head {
        BranchHead::Checkout => pick(
            others()
                .map(local)
                .chain(branches.remote.iter().map(remote))
                .take(LISTED)
                .collect(),
        ),
        BranchHead::Create { checkout } => {
            let here = Shown {
                name: "HEAD".into(),
                full: "HEAD".into(),
                remote: false,
            };
            let shown = (std::iter::once(here))
                .chain(branches.local.iter().map(local))
                .chain(branches.remote.iter().map(remote))
                .take(LISTED)
                .collect();
            (
                STARTS_LIST,
                "git-branch-start",
                Reads::Start { checkout, shown },
            )
        }
        BranchHead::Rename => {
            let first = (branches.local.iter()).filter(|name| Some(name.as_str()) == current);
            pick(first.chain(others()).map(local).take(LISTED).collect())
        }
        BranchHead::Delete => pick(others().map(local).take(LISTED).collect()),
    }
}
