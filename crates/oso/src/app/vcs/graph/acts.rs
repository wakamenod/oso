//! What a graph's keys do with its commits (ADR 0127, step 4b, Q10): marking two for a range or
//! several to pick, checking out a branch on a line or the commit itself, making a branch or a
//! tag there, cherry-picking and reverting, each run on the repository's run thread.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git: the
//! repository's run thread does, and judges again there whether something is stopped (Q6).
//!
//! - A key acts on the commit at the primary cursor; a pick takes the marked commits instead,
//!   if any, oldest first, and `RET` with two marked reviews the range between them.
//! - A detached checkout, a pick and a revert ask first, naming the commits; a prompt or a
//!   question counts only for the graph it was asked over, and one closed without its answer
//!   is forgotten. A merge's revert is refused, naming the shell's way (Q7).
//! - Only a branch name the push's filter takes is offered to check out, as step 3b's prompts.

use oso_core::complete::Candidate;
use oso_core::editor::{GraphAct, ScopeKind};
use oso_core::minibuffer::PromptId;

use super::super::super::App;
use super::super::runs::RUNNING;
use crate::commands::vcs::graph::GRAPH_LABELS_LIST;
use crate::vcs::branch::BranchRun;
use crate::vcs::graph::GraphRun;
use crate::vcs::graph::write::LineCommit;
use crate::vcs::run::RunRequest;

/// What a graph's prompt or question reads.
#[derive(Debug)]
pub(super) enum Reads {
    /// Which of the branches on a line to check out: each name, and whether it is a remote's.
    Label(Vec<(String, bool)>),
    /// The name of a branch made at `at`.
    BranchName { at: String },
    /// The name of a tag made at `at`, then its message if `annotated`.
    TagName { at: String, annotated: bool },
    /// The message of the tag `name` made at `at`.
    TagMessage { name: String, at: String },
    /// The `y` to checking out `id`, detached.
    Detach { id: String },
    /// The `y` to picking `ids`, oldest first.
    Pick { ids: Vec<String> },
    /// The `y` to reverting `id`.
    Revert { id: String },
}

/// A graph's prompt or question waiting for its answer.
#[derive(Debug)]
pub(super) struct Prompted {
    pub(super) prompt: PromptId,
    pub(super) reads: Reads,
}

/// A run a graph's key asks for: a branch's, or a graph's own.
enum GraphKeyRun {
    Branch(BranchRun),
    Graph(GraphRun),
}

impl App {
    /// What `act` asks of `commit`, the one at point, in the graph at `index`.
    pub(super) fn graph_commit_act(&mut self, index: usize, act: GraphAct, commit: LineCommit) {
        let short = commit.short.clone();
        match act {
            GraphAct::Mark => self.graph_mark(index, commit),
            GraphAct::Checkout => self.graph_checkout(index, commit),
            GraphAct::Branch => {
                let reads = Reads::BranchName { at: commit.id };
                self.graph_prompt(index, "git-graph-branch-name", reads);
            }
            GraphAct::Tag { annotated } => {
                let reads = Reads::TagName {
                    at: commit.id,
                    annotated,
                };
                self.graph_prompt(index, "git-graph-tag-name", reads);
            }
            GraphAct::CherryPick => self.graph_pick(index, commit),
            GraphAct::Revert if commit.merge => self.editor.message(format!(
                "{short} is a merge: git revert -m 1 {short} in a shell reverts it against its \
                 first parent"
            )),
            GraphAct::Revert => {
                let question = format!("Revert {short}? A new commit undoes its changes");
                self.graph_ask(index, &question, Reads::Revert { id: commit.id });
            }
            // Carried out before a line is looked at (`graph_act`).
            GraphAct::Review | GraphAct::CopyId | GraphAct::Refresh | GraphAct::Confirmed => {
                debug_assert!(false, "{act:?} is carried out before a line is looked at");
            }
        }
    }

    /// `m`: marks or unmarks `commit`, the graph's modeline saying how many are marked.
    fn graph_mark(&mut self, index: usize, commit: LineCommit) {
        let marks = &mut self.gits.graphs.open[index].marks;
        let said = if let Some(at) = marks.iter().position(|id| *id == commit.id) {
            marks.remove(at);
            format!("Unmarked {}", commit.short)
        } else {
            marks.push(commit.id);
            format!("Marked {}", commit.short)
        };
        let count = marks.len();
        self.set_graph_note(index);
        self.editor.message(format!("{said} ({count} marked)"));
    }

    /// `RET` with two commits marked: reviews the range between them, the older first, as
    /// `review-range` reads `FROM..TO`.
    pub(super) fn review_marked(&mut self, index: usize) {
        let graph = &self.gits.graphs.open[index];
        let Some(range) = graph.built.range(&graph.marks) else {
            return self
                .editor
                .message("A marked commit is no longer shown: m marks again");
        };
        let project = graph.project;
        self.open_review(project, ScopeKind::Range, Some(range));
    }

    /// `c`: the branch whose label is on the commit's line, asking which if several, else the
    /// commit, detached, after a question.
    fn graph_checkout(&mut self, index: usize, commit: LineCommit) {
        let branches = commit.branches();
        match branches.as_slice() {
            [] => {
                let question = format!(
                    "Check out {}, detached? What is committed there is on no branch until one \
                     is made",
                    commit.short
                );
                self.graph_ask(index, &question, Reads::Detach { id: commit.id });
            }
            [(name, remote)] => {
                let run = BranchRun::Checkout {
                    name: name.clone(),
                    remote: *remote,
                };
                self.graph_run(index, GraphKeyRun::Branch(run));
            }
            _ => {
                let items: std::sync::Arc<[Candidate]> = (branches.iter())
                    .map(|(name, remote)| {
                        Candidate::new(name.as_str(), "", if *remote { "remote" } else { "" })
                    })
                    .collect();
                self.editor.set_listed(GRAPH_LABELS_LIST, items);
                self.graph_prompt(index, "git-graph-label", Reads::Label(branches));
            }
        }
    }

    /// `C`: the marked commits, oldest first, or `commit`, after a question naming them.
    fn graph_pick(&mut self, index: usize, commit: LineCommit) {
        let graph = &self.gits.graphs.open[index];
        let mut picked = graph.built.oldest_first(&graph.marks);
        if picked.is_empty() {
            picked.push((commit.id, commit.short));
        }
        let shorts: Vec<&str> = picked.iter().map(|(_, short)| short.as_str()).collect();
        let what = match shorts.as_slice() {
            [one] => (*one).to_owned(),
            all => format!("{} commits, oldest first: {}", all.len(), all.join(", ")),
        };
        let question = format!("Cherry-pick {what} onto HEAD?");
        let ids = picked.into_iter().map(|(id, _)| id).collect();
        self.graph_ask(index, &question, Reads::Pick { ids });
    }

    /// What the graph's prompt read as `text`.
    pub(super) fn graph_answered(&mut self, index: usize, text: &str) {
        let text = text.trim().to_owned();
        let Some(Prompted { reads, .. }) = self.gits.graphs.open[index].wait.take() else {
            return self.editor.message("The graph's prompt is gone: ask again");
        };
        if text.is_empty() && !matches!(reads, Reads::TagMessage { .. }) {
            return self.editor.message("Nothing was named");
        }
        let run = match reads {
            Reads::Label(branches) => match branches.into_iter().find(|(name, _)| *name == text) {
                Some((name, remote)) => GraphKeyRun::Branch(BranchRun::Checkout { name, remote }),
                None => return self.editor.message("That branch is not on the line"),
            },
            Reads::BranchName { at } => GraphKeyRun::Branch(BranchRun::Create {
                name: text,
                start: at,
                checkout: false,
            }),
            Reads::TagName {
                at,
                annotated: true,
            } => {
                let reads = Reads::TagMessage { name: text, at };
                return self.graph_prompt(index, "git-graph-tag-message", reads);
            }
            Reads::TagName {
                at,
                annotated: false,
            } => GraphKeyRun::Graph(GraphRun::Tag {
                name: text,
                at,
                message: None,
            }),
            Reads::TagMessage { name, at } => GraphKeyRun::Graph(GraphRun::Tag {
                name,
                at,
                message: Some(text),
            }),
            Reads::Detach { .. } | Reads::Pick { .. } | Reads::Revert { .. } => {
                return self
                    .editor
                    .message("The graph's question is gone: ask again");
            }
        };
        self.graph_run(index, run);
    }

    /// The `y` to the graph's question.
    pub(super) fn graph_confirmed(&mut self, index: usize) {
        let Some(Prompted { reads, .. }) = self.gits.graphs.open[index].wait.take() else {
            return self
                .editor
                .message("The graph's question is gone: ask again");
        };
        let (run, pick) = match reads {
            Reads::Detach { id } => (GraphKeyRun::Branch(BranchRun::Detach { id }), false),
            Reads::Pick { ids } => (GraphKeyRun::Graph(GraphRun::Pick { ids }), true),
            Reads::Revert { id } => (GraphKeyRun::Graph(GraphRun::Revert { id }), false),
            _ => {
                return self
                    .editor
                    .message("The graph's question is gone: ask again");
            }
        };
        // The marks go once their pick has begun.
        if self.graph_run(index, run) && pick {
            self.gits.graphs.open[index].marks.clear();
            self.set_graph_note(index);
        }
    }

    /// Opens `command`'s prompt over the graph at `index`, and keeps it for `reads`.
    fn graph_prompt(&mut self, index: usize, command: &str, reads: Reads) {
        if let Some(prompt) = self.open_host_prompt(command) {
            self.gits.graphs.open[index].wait = Some(Prompted { prompt, reads });
        }
    }

    /// Asks `question` over the graph at `index`, its `y` running what `reads` names.
    fn graph_ask(&mut self, index: usize, question: &str, reads: Reads) {
        if let Some(prompt) = self.ask_bound(question, "git-graph-confirmed", ["yes", "keep"]) {
            self.gits.graphs.open[index].wait = Some(Prompted { prompt, reads });
        }
    }

    /// Starts `run` in the repository of the graph at `index`; whether it began.
    fn graph_run(&mut self, index: usize, run: GraphKeyRun) -> bool {
        let graph = &self.gits.graphs.open[index];
        let (project, root) = (graph.project, graph.root.clone());
        let repo = self.git_repo(project, &root);
        if self.repo_busy(project) {
            self.editor.message(RUNNING);
            return false;
        }
        let pass = self.pass_env();
        let (request, title, done) = match run {
            GraphKeyRun::Branch(run) => {
                let (title, done) = (run.title(), run.done());
                (RunRequest::Branch { run, pass }, title, done)
            }
            GraphKeyRun::Graph(run) => {
                let (title, done) = (run.title(), run.done());
                (RunRequest::Graph { run, pass }, title, done)
            }
        };
        let began = self.ask_runner(repo, request);
        if began {
            self.run_began(repo, title, done);
        }
        began
    }

    /// Says in the graph's modeline how many commits are marked, or nothing.
    pub(super) fn set_graph_note(&mut self, index: usize) {
        let graph = &self.gits.graphs.open[index];
        let count = graph.marks.len();
        let note = if count == 0 {
            String::new()
        } else {
            format!("{count} marked")
        };
        self.editor.set_mode_note(graph.buffer, note);
    }
}
