//! The tree's projects in the app (ADR 0131, step 2): each repository of an open space, in the
//! order of the spaces and then their repositories (ADR 0136, step 1a), and its worktrees as
//! git last listed them in a trusted project, or else the worktrees the spaces hold in it. A listing is a job on the
//! `oso-sessions-git` threads, asked when a project first shows, on `g`, and when the sessions
//! worker sees the project's `worktrees` directory change.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! Nothing here touches the disk: the worker `stat`s and the jobs run git. A project's trust is
//! found through a buffer of it, a file's or a session's, as worktree sessions find it.

mod mouse;
mod rows;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use oso_core::project::ProjectId;

use crate::app::App;
use crate::sessions::list::{Project, Tree, Worktree};
use crate::sessions::worker::Job;

/// What the app keeps of the tree's projects.
#[derive(Debug, Default)]
pub(super) struct Pane {
    /// Each project's worktrees as git last listed them, by the project's root.
    listed: HashMap<PathBuf, Vec<Worktree>>,
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
        let projects = roots
            .iter()
            .map(|root| {
                let held = held.get(root).unwrap_or(&none);
                self.ask_tree_listing(root, held, &trusted);
                Project {
                    root: root.clone(),
                    name: crate::sessions::spaces::stored::last_component(root),
                    worktrees: self.worktrees_of(root, held),
                }
            })
            .collect();
        self.watch_projects(&roots);
        // The worktree of the session the space in front showed last.
        let front = self
            .watch
            .spaces
            .current()
            .and_then(|at| self.watch.spaces.get(at)?.last_session)
            .and_then(|buffer| self.hosting.table().by_buffer(buffer))
            .map(|session| super::place(session).to_path_buf());
        Tree {
            projects,
            front,
            wall: Some(SystemTime::now()),
        }
    }

    /// `root`'s worktrees: as listed, else its main checkout and `held`, the worktrees spaces
    /// hold in it.
    fn worktrees_of(&self, root: &Path, held: &[PathBuf]) -> Vec<Worktree> {
        if let Some(listed) = self.watch.view.pane.listed.get(root) {
            return listed.clone();
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
        self.worktree_jobs().tree_worktrees(trust, root.to_owned());
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
            self.relist(root);
        }
        self.watch.stale = true;
    }

    /// `g`: every project is listed again.
    pub(super) fn relist_projects(&mut self) {
        let roots: Vec<PathBuf> = self.watch.view.pane.listed.keys().cloned().collect();
        for root in roots {
            self.relist(&root);
        }
        self.watch.stale = true;
    }

    /// Lists `root` again, keeping its rows until the new listing comes.
    fn relist(&mut self, root: &Path) {
        if self.watch.view.pane.asked.contains(root) {
            return;
        }
        if let Some(trust) = self.trusted_project(root) {
            self.watch.view.pane.asked.insert(root.to_owned());
            self.worktree_jobs().tree_worktrees(trust, root.to_owned());
        }
    }
}
