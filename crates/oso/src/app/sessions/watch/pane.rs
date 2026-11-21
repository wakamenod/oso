//! The tree's spaces in the app (ADR 0136, step 2): each open space, its repositories, and the
//! worktrees `space_rows` chooses for it of each repository's, as git last listed them in a
//! trusted project, or else the worktrees the spaces hold in it; and the closed spaces. A
//! listing is a job on the `oso-sessions-git` threads, asked when a project first shows, on
//! `g`, and when the sessions worker sees the project's `worktrees` directory change.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! Nothing here touches the disk: the worker `stat`s and the jobs run git. A project's trust is
//! found through a buffer of it, a file's or a session's, as worktree sessions find it.

mod mouse;
mod rows;

use crate::sessions::worktree::git::{OpenPull, pull_line};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use oso_core::project::ProjectId;

use crate::app::App;
use crate::sessions::list::{ClosedSpace, SpaceInput, SpaceRows, Tree, Worktree, space_rows};
use crate::sessions::worker::Job;

/// What the app keeps of the tree's projects.
#[derive(Debug, Default)]
pub(super) struct Pane {
    /// Each project's worktrees as git last listed them, by the project's root.
    listed: HashMap<PathBuf, Vec<Worktree>>,
    /// When each repository's open pull requests were last asked for, and what they were: kept
    /// apart from the listing, so that a listing that reads none keeps them (ADR 0127, 5b-2).
    pulls_read: HashMap<PathBuf, std::time::Instant>,
    pulls: HashMap<PathBuf, Vec<OpenPull>>,
    /// Projects whose listing was asked for and has not come.
    asked: HashSet<PathBuf>,
    /// The `worktrees` directories the worker watches, as last told.
    watched: Vec<(PathBuf, PathBuf)>,
}

impl App {
    /// What the tree shows besides sessions, as of now: its projects, the worktree in front and
    /// the wall-clock time. Asks for each trusted project's listing that is missing. The held
    /// worktrees and the trusted projects of buffers are gathered once a call, as it runs with
    /// each frame of the spinner.
    pub(super) fn sessions_tree(&mut self) -> Tree {
        let mut roots: Vec<PathBuf> = Vec::new();
        for space in self.watch.spaces.iter().filter(|space| space.is_open()) {
            for member in &space.stored.members {
                if !roots.contains(&member.root) {
                    roots.push(member.root.clone());
                }
            }
        }
        let held = self.held_worktrees();
        let none: Vec<PathBuf> = Vec::new();
        let unlisted = roots
            .iter()
            .any(|root| !self.watch.view.pane.listed.contains_key(root));
        let trusted = if unlisted {
            self.trusted_projects()
        } else {
            HashMap::new()
        };
        let mut all: HashMap<PathBuf, Vec<Worktree>> = HashMap::new();
        for root in &roots {
            let held = held.get(root).unwrap_or(&none);
            self.ask_tree_listing(root, held, &trusted);
            all.insert(root.clone(), self.worktrees_of(root, held));
        }
        self.watch_projects(&roots);
        let spaces = self.tree_spaces(&all);
        let closed = (self.watch.spaces.iter())
            .filter(|space| !space.is_open())
            .map(|space| ClosedSpace {
                id: space.id().clone(),
                name: space.label().to_owned(),
                // Its start alone: a closed space's row shows a few cells of it, on every frame.
                title: crate::sessions::spaces::stored::cut_bytes(
                    &space.stored.description,
                    crate::sessions::spaces::stored::MAX_OLD_TITLE,
                )
                .to_owned(),
            })
            .collect();
        let front_space = (self.watch.spaces.current())
            .and_then(|at| self.watch.spaces.get(at))
            .map(|space| space.id().clone());
        // The worktree of the session the space in front showed last.
        let front = self
            .watch
            .spaces
            .current()
            .and_then(|at| self.watch.spaces.get(at)?.last_session)
            .and_then(|buffer| self.hosting.table().by_buffer(buffer))
            .map(|session| super::place(session).to_path_buf());
        Tree {
            spaces,
            closed,
            front_space,
            front,
            wall: Some(SystemTime::now()),
        }
    }

    /// Each open space's repositories with the worktrees it shows of `all`, each repository's
    /// worktrees, as `space_rows` chooses them (ADR 0136, step 2).
    fn tree_spaces(&self, all: &HashMap<PathBuf, Vec<Worktree>>) -> Vec<SpaceRows> {
        // Where each space's sessions are, running or stopped, by the space's place.
        let mut sessions: HashMap<usize, HashSet<PathBuf>> = HashMap::new();
        for session in self.hosting.table().iter().filter(|s| !s.archived) {
            if let Some(at) = self.space_of_session(session) {
                let place = super::place(session).to_path_buf();
                sessions.entry(at).or_default().insert(place);
            }
        }
        let (mut open, mut closed) = (Vec::new(), Vec::new());
        for (at, space) in self.watch.spaces.iter().enumerate() {
            let input = SpaceInput {
                id: space.id(),
                name: space.label(),
                members: (space.stored.members.iter())
                    .map(|member| member.root.as_path())
                    .collect(),
                held: &space.stored.worktrees,
                sessions: sessions.remove(&at).unwrap_or_default(),
                directory: space.stored.directory.as_deref(),
            };
            if space.is_open() {
                open.push(input);
            } else {
                closed.push(input);
            }
        }
        let mut rows = space_rows(&open, &closed, all);
        // Each space's orchestrator by the record's id, never by a session's name (step 5).
        for row in &mut rows {
            let spaces = &self.watch.spaces;
            let recorded = (spaces.find(&row.id))
                .and_then(|at| spaces.stored(at))
                .and_then(|stored| stored.orchestrator.as_ref());
            row.orchestrator = recorded.and_then(|id| {
                let session = self.hosting.table().iter().find(|s| &s.id == id)?;
                Some(crate::sessions::list::Key::Hosted(session.key))
            });
        }
        rows
    }

    /// `root`'s worktrees: as listed, else its main checkout and `held`, the worktrees spaces
    /// hold in it.
    fn worktrees_of(&self, root: &Path, held: &[PathBuf]) -> Vec<Worktree> {
        let pane = &self.watch.view.pane;
        if let Some(listed) = pane.listed.get(root) {
            let pulls = pane.pulls.get(root).map_or(&[][..], Vec::as_slice);
            let mut listed = listed.clone();
            for worktree in &mut listed {
                worktree.pull = pull_line(pulls, worktree.branch.as_deref());
            }
            return listed;
        }
        let main = Worktree {
            path: root.to_owned(),
            primary: true,
            ..Worktree::default()
        };
        let linked = held.iter().map(|path| Worktree {
            path: path.clone(),
            ..Worktree::default()
        });
        std::iter::once(main).chain(linked).collect()
    }

    /// The worktrees open spaces hold inside each of their repositories, as oso makes them, by
    /// the repository's root: one pass over the spaces.
    fn held_worktrees(&self) -> HashMap<PathBuf, Vec<PathBuf>> {
        let mut held: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        let mut seen: HashSet<(&Path, &Path)> = HashSet::new();
        for space in self.watch.spaces.iter().filter(|space| space.is_open()) {
            for member in &space.stored.members {
                let root = member.root.as_path();
                for path in &space.stored.worktrees {
                    if path.starts_with(root) && path != root && seen.insert((root, path)) {
                        held.entry(root.to_owned()).or_default().push(path.clone());
                    }
                }
            }
        }
        held
    }

    /// The trusted projects buffers are in, by their roots: one pass over the buffers.
    fn trusted_projects(&self) -> HashMap<PathBuf, ProjectId> {
        let mut trusted = HashMap::new();
        for id in self
            .editor
            .buffers()
            .filter_map(|(_, buffer)| buffer.project())
        {
            if self.editor.project_trusted(id)
                && let Some(project) = self.editor.project(id)
            {
                trusted.entry(project.root().to_owned()).or_insert(id);
            }
        }
        trusted
    }

    /// Every linked worktree git listed, as its components spell it.
    pub(in crate::app) fn listed_worktrees(&self) -> HashSet<PathBuf> {
        let same = oso_core::project::same_place;
        let mut worktrees = HashSet::new();
        for (root, listed) in &self.watch.view.pane.listed {
            for worktree in listed.iter().filter(|w| !same(root, &w.path)) {
                worktrees.insert(worktree.path.components().collect());
            }
        }
        worktrees
    }

    /// The linked worktrees git last listed of the repository at `root`, none if it was not
    /// listed (ADR 0163).
    pub(in crate::app) fn listed_worktrees_of(&self, root: &Path) -> Vec<PathBuf> {
        let same = oso_core::project::same_place;
        let pane = &self.watch.view.pane;
        let listed = (pane.listed.iter()).find(|(listed, _)| same(listed, root));
        listed.map_or_else(Vec::new, |(_, worktrees)| {
            (worktrees.iter())
                .filter(|w| !same(root, &w.path))
                .map(|w| w.path.clone())
                .collect()
        })
    }

    /// The worktrees the open spaces hold in the repository at `root` (ADR 0163).
    pub(in crate::app) fn held_worktrees_in(&self, root: &Path) -> Vec<PathBuf> {
        let same = oso_core::project::same_place;
        let held = self.held_worktrees();
        (held.into_iter())
            .filter(|(repository, _)| same(repository, root))
            .flat_map(|(_, paths)| paths)
            .collect()
    }

    /// The repository git listed `place` as a linked worktree of, if it did.
    pub(in crate::app) fn listed_repository_of(&self, place: &Path) -> Option<PathBuf> {
        let same = oso_core::project::same_place;
        self.watch
            .view
            .pane
            .listed
            .iter()
            .find(|(root, worktrees)| {
                !same(root, place) && worktrees.iter().any(|w| same(&w.path, place))
            })
            .map(|(root, _)| root.clone())
    }

    /// Asks for `root`'s listing if it is trusted, through a buffer of it or of one of `held`,
    /// by `trusted`, and neither listed nor asked for.
    fn ask_tree_listing(
        &mut self,
        root: &Path,
        held: &[PathBuf],
        trusted: &HashMap<PathBuf, ProjectId>,
    ) {
        let pane = &self.watch.view.pane;
        if pane.listed.contains_key(root) || pane.asked.contains(root) {
            return;
        }
        let project = std::iter::once(root)
            .chain(held.iter().map(PathBuf::as_path))
            .find_map(|path| trusted.get(path).copied());
        let Some(trust) = project.and_then(|project| self.trust_for(project).ok()) else {
            return;
        };
        self.watch.view.pane.asked.insert(root.to_owned());
        let pulls = self.pulls_due(root);
        self.worktree_jobs()
            .tree_worktrees(trust, root.to_owned(), pulls);
    }

    /// The trust a job for the repository at `root` runs under: through a buffer of it, or of
    /// one of its worktrees, in a project the editor trusts.
    fn trusted_project(&self, root: &Path) -> Option<crate::sessions::worktree::jobs::Trust> {
        self.trusted_project_of(root)
            .and_then(|project| self.trust_for(project).ok())
    }

    /// A trusted project of a buffer in the repository at `root` or in one of the worktrees
    /// spaces hold in it.
    pub(in crate::app) fn trusted_project_of(&self, root: &Path) -> Option<ProjectId> {
        let held = self.held_worktrees();
        let held = held.get(root).map_or(&[][..], Vec::as_slice);
        let trusted = self.trusted_projects();
        std::iter::once(root)
            .chain(held.iter().map(PathBuf::as_path))
            .find_map(|path| trusted.get(path).copied())
    }

    /// Tells the worker which projects' `worktrees` directories to watch, when that changed.
    fn watch_projects(&mut self, roots: &[PathBuf]) {
        let watched: Vec<(PathBuf, PathBuf)> = roots
            .iter()
            .filter(|root| self.watch.view.pane.listed.contains_key(*root))
            .map(|root| (root.clone(), root.join(".git").join("worktrees")))
            .collect();
        if watched == self.watch.view.pane.watched {
            return;
        }
        self.watch.view.pane.watched.clone_from(&watched);
        if let Some(worker) = &self.sessions {
            worker.send(Job::Watch(watched));
        }
    }

    /// A listing for the tree came: it shows from the next drawing. A failed one leaves the
    /// worktrees spaces hold, and is said in the log only.
    pub(in crate::app) fn tree_listed(
        &mut self,
        root: PathBuf,
        result: Result<Vec<Worktree>, String>,
    ) {
        let pane = &mut self.watch.view.pane;
        pane.asked.remove(&root);
        match result {
            Ok(worktrees) => {
                pane.listed.insert(root, worktrees);
            }
            Err(why) => log::info!("the tree's worktrees were not listed: {why}"),
        }
        self.watch.stale = true;
    }

    /// The worker saw these projects' worktrees change: each is listed again.
    pub(in crate::app) fn worktrees_moved(&mut self, roots: &[PathBuf]) {
        for root in roots {
            // A change the worker saw may be an agent's: it never asks `gh`.
            self.relist(root, false);
        }
        self.watch.stale = true;
    }

    /// The open pull requests of the tree's repository at `root` came.
    pub(in crate::app) fn tree_pulls(&mut self, root: PathBuf, pulls: Vec<OpenPull>) {
        self.watch.view.pane.pulls.insert(root, pulls);
        self.watch.stale = true;
    }

    /// Whether `root`'s open pull requests are asked for with its worktrees now: not for an
    /// agent's doing, and at most once a minute (ADR 0127, step 5b-2).
    fn pulls_due(&mut self, root: &Path) -> bool {
        if self.gits.by_agent {
            return false;
        }
        let last = self.watch.view.pane.pulls_read.get(root);
        if last.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(60)) {
            return false;
        }
        self.watch
            .view
            .pane
            .pulls_read
            .insert(root.to_owned(), std::time::Instant::now());
        true
    }

    /// `g`: every project is listed again.
    pub(super) fn relist_projects(&mut self) {
        let roots: Vec<PathBuf> = self.watch.view.pane.listed.keys().cloned().collect();
        for root in roots {
            self.relist(&root, true);
        }
        self.watch.stale = true;
    }

    /// Lists `root` again, keeping its rows until the new listing comes, with its open pull
    /// requests if `pulls` and they are due: the person's `g`, never the worker's change.
    fn relist(&mut self, root: &Path, pulls: bool) {
        if self.watch.view.pane.asked.contains(root) {
            return;
        }
        if let Some(trust) = self.trusted_project(root) {
            self.watch.view.pane.asked.insert(root.to_owned());
            let pulls = pulls && self.pulls_due(root);
            self.worktree_jobs()
                .tree_worktrees(trust, root.to_owned(), pulls);
        }
    }
}
