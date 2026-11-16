//! Graph buffers in the app (ADR 0127, step 4): showing a repository's commits as a graph, one
//! buffer per repository (Q8), each with a thread of its own, its pages as point nears the
//! end, and what the keys on its lines ask.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or lays out a
//! text: each graph buffer's thread, `oso-vcs-graph`, does, and its text goes in at O(1), with
//! what it replaces freed off the UI thread.
//!
//! - A graph runs git, so only for a trusted project; nothing runs while it is not shown, but a
//!   read the person asked for.
//! - The next page is asked once point comes within [`NEARS_END`] lines of the end, after a key
//!   of the person's in the graph, never an agent's command (ADR 0085), while git has more, up
//!   to the graph's most (Q4). A page that fails ends the paging until the graph is read again.
//! - A project no longer trusted has its graph's root revoked at once, so that no read queued
//!   on its thread runs git.
//! - A text waits while the editor's text must not change, as a status's does; one that comes
//!   for an older request, or for a graph gone, is freed off the UI thread.
//! - It is read again after each run oso does in its repository while it is shown, and on `g`.

use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::editor::{GitRequest, GitShown, GraphAct, GraphScope};
use oso_core::project::ProjectId;

use super::super::{App, UserEvent};
use crate::commands::vcs::graph::RESTRICTED;
use crate::git::TrustedRoot;
use crate::vcs::graph::work::{GraphReport, GraphRequest, GraphWorker};
use crate::vcs::graph::write::{Built, Laid};

/// How near the end point comes before the next page is read.
const NEARS_END: usize = 1000;

/// What a graph buffer's thread sent the UI thread, as its `serial`th start.
pub(crate) struct GraphEvent {
    buffer: BufferId,
    serial: u64,
    report: GraphReport,
}

impl std::fmt::Debug for GraphEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphEvent")
            .field("serial", &self.serial)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

/// What a graph shows: the branch checked out, every ref, or one named, in full.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shows {
    Current,
    All,
    Named(String),
}

impl Shows {
    /// The refs `git log` is given, and the heading.
    fn refs(&self) -> (Vec<String>, String) {
        match self {
            Self::Current => (vec!["HEAD".into()], "Commits of HEAD".into()),
            Self::All => (
                ["--branches", "--remotes", "--tags", "HEAD"]
                    .map(str::to_owned)
                    .to_vec(),
                "Commits of every branch, remote branch and tag".into(),
            ),
            Self::Named(full) => {
                let shown = (full.strip_prefix("refs/heads/"))
                    .or_else(|| full.strip_prefix("refs/remotes/"))
                    .or_else(|| full.strip_prefix("refs/tags/"))
                    .unwrap_or(full);
                (vec![full.clone()], format!("Commits of {shown}"))
            }
        }
    }
}

/// A graph buffer as the app knows it.
struct Graph {
    buffer: BufferId,
    project: ProjectId,
    root: TrustedRoot,
    serial: u64,
    worker: Option<GraphWorker>,
    shows: Shows,
    /// The newest request's generation; a report of an older one is dropped.
    asked: u64,
    /// A page is being read.
    loading: bool,
    /// The next text puts point on the first commit, as after a read from the first page.
    fresh: bool,
    /// A page failed: no other is asked until the graph is read again.
    stopped: bool,
    built: Arc<Built>,
    /// The newest text, held while the editor's text must not change.
    waiting: Option<Laid>,
}

/// The graph buffers the app keeps.
#[derive(Default)]
pub(super) struct Graphs {
    open: Vec<Graph>,
    serial: u64,
    generation: u64,
}

impl App {
    /// Carries out what a graph's commands asked.
    pub(super) fn git_graph_request(&mut self, request: GitRequest) {
        match request {
            GitRequest::Graph { project, scope } => {
                let shows = match scope {
                    GraphScope::Current => Shows::Current,
                    GraphScope::All => Shows::All,
                    GraphScope::Pick => return self.pick_graph_ref(project),
                    GraphScope::Named(text) => match self.graph_ref_named(project, &text) {
                        Some(full) => Shows::Named(full),
                        None => return,
                    },
                };
                self.show_graph(project, shows);
            }
            GitRequest::GraphToggle { buffer } => {
                let Some(index) = self.graph_index(buffer) else {
                    return;
                };
                let graph = &self.gits.graphs.open[index];
                let shows = if graph.shows == Shows::All {
                    Shows::Current
                } else {
                    Shows::All
                };
                self.show_graph(graph.project, shows);
            }
            GitRequest::GraphAct { buffer, act, line } => self.graph_act(buffer, act, line),
            // The others are not a graph's (`requests::concern`).
            _ => {}
        }
    }

    /// Shows the graph of `project` that `shows` names in the selected window, making its
    /// buffer if it has none, and reads it from the first page.
    fn show_graph(&mut self, project: ProjectId, shows: Shows) {
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(RESTRICTED);
        };
        // A graph buffer whose record the app let go, as its trust was taken back, is used again.
        let kept = self.editor.git_graph_buffer(project);
        let index = kept.and_then(|buffer| self.graph_index(buffer));
        let index = index.unwrap_or_else(|| {
            let buffer = kept.unwrap_or_else(|| {
                let name = format!("*graph: {}*", self.project_name(project));
                self.editor.add_git_graph(project, name)
            });
            self.gits.graphs.open.push(Graph {
                buffer,
                project,
                root: root.clone(),
                serial: 0,
                worker: None,
                shows: Shows::Current,
                asked: 0,
                loading: false,
                fresh: true,
                stopped: false,
                built: Arc::default(),
                waiting: None,
            });
            self.gits.graphs.open.len() - 1
        });
        let graph = &mut self.gits.graphs.open[index];
        if graph.root.revoked() {
            graph.root = root;
            graph.worker = None;
        }
        graph.shows = shows;
        let buffer = graph.buffer;
        self.editor.show(buffer);
        self.read_graph(index);
        self.request_redraw();
    }

    /// Asks the graph at `index` to read its first page again, if its project is still trusted.
    fn read_graph(&mut self, index: usize) {
        if !self
            .editor
            .project_trusted(self.gits.graphs.open[index].project)
        {
            self.gits.graphs.open[index].root.revoke();
            return self.editor.message(RESTRICTED);
        }
        if self.gits.graphs.open[index].worker.is_none() {
            self.start_graph_worker(index);
        }
        self.gits.graphs.generation += 1;
        let generation = self.gits.graphs.generation;
        let graph = &mut self.gits.graphs.open[index];
        let (refs, title) = graph.shows.refs();
        graph.asked = generation;
        graph.fresh = true;
        graph.stopped = false;
        let asked = (graph.worker.as_ref()).is_some_and(|worker| {
            worker.ask(GraphRequest::Read {
                generation,
                refs,
                title,
            })
        });
        graph.loading = asked;
        if !asked {
            // The next read starts a thread again.
            graph.worker = None;
            self.editor
                .message("The graph's thread had ended: g reads it again");
        }
    }

    fn start_graph_worker(&mut self, index: usize) {
        self.gits.graphs.serial += 1;
        let serial = self.gits.graphs.serial;
        let graph = &mut self.gits.graphs.open[index];
        graph.serial = serial;
        let buffer = graph.buffer;
        let waker = self.waker.clone();
        let started = GraphWorker::start(
            graph.root.clone(),
            super::super::reviews::state(),
            move |report| {
                waker.send(UserEvent::Graph(GraphEvent {
                    buffer,
                    serial,
                    report,
                }))
            },
        );
        match started {
            Ok(worker) => self.gits.graphs.open[index].worker = Some(worker),
            Err(e) => self
                .editor
                .message(format!("Cannot start the graph's thread: {e}")),
        }
    }

    /// A graph buffer's thread reported.
    pub(in crate::app) fn graph_reported(&mut self, event: GraphEvent) {
        let GraphEvent {
            buffer,
            serial,
            report,
        } = event;
        let Some(index) = self.graph_index(buffer) else {
            return self.freer.free(report);
        };
        let graph = &mut self.gits.graphs.open[index];
        if graph.serial != serial {
            return self.freer.free(report);
        }
        match report {
            GraphReport::Laid { generation, laid } if generation == graph.asked => {
                graph.loading = false;
                if self.editor.reload_waits() {
                    let old = graph.waiting.replace(laid);
                    self.freer.free(old);
                } else {
                    self.put_in_graph(index, laid);
                }
            }
            GraphReport::Unchanged { generation } if generation == graph.asked => {
                graph.loading = false;
            }
            GraphReport::Failed { generation, why } if generation == graph.asked => {
                graph.loading = false;
                graph.stopped = true;
                let why = crate::commands::review::one_line(&why, super::MAX_MESSAGE);
                self.editor.message(format!("git: {why}"));
            }
            // An older request's: a newer one is under way.
            report => self.freer.free(report),
        }
        self.request_redraw();
    }

    fn put_in_graph(&mut self, index: usize, laid: Laid) {
        let Laid {
            text,
            highlights,
            built,
        } = laid;
        let graph = &mut self.gits.graphs.open[index];
        let buffer = graph.buffer;
        let (line, lines) = if std::mem::take(&mut graph.fresh) {
            (1, Vec::new())
        } else {
            let lines = self.editor.git_points(buffer);
            (lines.first().map_or(1, |(_, line)| *line), lines)
        };
        let old_text =
            (self.editor.buffer(buffer)).map(|shown| (shown.text().clone(), shown.display()));
        let shown = GitShown {
            highlights,
            outline: Vec::new(),
            line,
            lines,
        };
        let refused = self.editor.set_git_text(buffer, text, shown);
        let old = std::mem::replace(&mut self.gits.graphs.open[index].built, built);
        self.freer.free((old, old_text, refused));
        self.buffers_changed();
    }

    /// What `act` asks of the commit on `line` of the graph `buffer`.
    fn graph_act(&mut self, buffer: BufferId, act: GraphAct, line: usize) {
        let Some(index) = self.graph_index(buffer) else {
            return;
        };
        if act == GraphAct::Refresh {
            return self.read_graph(index);
        }
        let graph = &self.gits.graphs.open[index];
        let Some(id) = graph.built.commit_at(line).map(str::to_owned) else {
            return self.editor.message("No commit on this line");
        };
        match act {
            GraphAct::Review => {
                let project = graph.project;
                self.open_review(project, oso_core::editor::ScopeKind::Commit, Some(id));
            }
            GraphAct::CopyId => {
                self.editor.kill_new(&id, false);
                self.write_clipboard();
                self.editor.message(format!("Copied {id}"));
            }
            GraphAct::Refresh => {}
        }
    }

    /// After each event: forgets killed graphs and those no longer trusted, revoking their
    /// roots, puts in a text that waited, and asks the next page of the graph `keyed`, the one a
    /// key of the person's was typed in, if its point nears the end.
    pub(super) fn sync_graphs(&mut self, keyed: Option<BufferId>) {
        let editor = &self.editor;
        let gone: Vec<usize> = (self.gits.graphs.open.iter().enumerate())
            .filter(|(_, graph)| {
                editor.git_graph_of(graph.buffer).is_none()
                    || !editor.project_trusted(graph.project)
            })
            .map(|(index, _)| index)
            .collect();
        for index in gone.into_iter().rev() {
            let graph = self.gits.graphs.open.remove(index);
            // What is queued on its thread finds the root revoked, and runs no git.
            graph.root.revoke();
            self.freer.free((graph.built, graph.waiting));
        }
        if !self.editor.reload_waits() {
            for index in 0..self.gits.graphs.open.len() {
                if let Some(laid) = self.gits.graphs.open[index].waiting.take() {
                    self.put_in_graph(index, laid);
                    self.request_redraw();
                }
            }
        }
        let Some(selected) = keyed.filter(|buffer| *buffer == self.editor.window().buffer()) else {
            return;
        };
        let Some(index) = self.graph_index(selected) else {
            return;
        };
        let graph = &self.gits.graphs.open[index];
        if graph.loading || graph.stopped || !graph.built.more {
            return;
        }
        let rows = graph.built.lines.len();
        let near =
            (self.editor.git_points(selected).iter()).any(|(_, line)| line + NEARS_END >= rows);
        if !near {
            return;
        }
        let generation = graph.asked;
        let graph = &mut self.gits.graphs.open[index];
        graph.loading = (graph.worker.as_ref())
            .is_some_and(|worker| worker.ask(GraphRequest::More { generation }));
    }

    /// A run oso did in `project`'s repository ended: its graph, if shown, is read again.
    pub(super) fn graph_after_run(&mut self, project: ProjectId) {
        let Some(index) = (self.gits.graphs.open.iter()).position(|graph| graph.project == project)
        else {
            return;
        };
        let buffer = self.gits.graphs.open[index].buffer;
        if self
            .editor
            .windows()
            .any(|window| window.buffer() == buffer)
        {
            self.read_graph(index);
        }
    }

    fn graph_index(&self, buffer: BufferId) -> Option<usize> {
        (self.gits.graphs.open.iter()).position(|graph| graph.buffer == buffer)
    }
}
