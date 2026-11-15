//! Which worktrees each space of the tree shows (ADR 0136, step 2): those it holds; the main
//! checkout while a session of the space is there, running or stopped; and in the first space
//! that has the repository, the main checkout and every worktree no space holds. A worktree is
//! held only by a space, open or closed, that still has its repository, so one a closed space
//! holds shows in no open one. So a worktree shows in one space only, but for a main checkout a
//! session of another space is in.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data in, rows out.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::{Project, SpaceRows, Worktree};
use crate::sessions::spaces::SpaceId;

/// What the tree needs of an open space, in the spaces' order.
#[derive(Debug)]
pub struct SpaceInput<'a> {
    pub id: &'a SpaceId,
    pub name: &'a str,
    /// Its repositories' roots, in its order.
    pub members: Vec<&'a Path>,
    /// The worktrees it holds.
    pub held: &'a [PathBuf],
    /// Where its sessions are, running or stopped, archived ones aside.
    pub sessions: HashSet<PathBuf>,
    /// Its own directory.
    pub directory: Option<&'a Path>,
}

/// The rows of the open `spaces`, each repository's worktrees taken from `listed`; `closed`
/// spaces' worktrees show in none of them.
pub fn space_rows(
    spaces: &[SpaceInput<'_>],
    closed: &[SpaceInput<'_>],
    listed: &HashMap<PathBuf, Vec<Worktree>>,
) -> Vec<SpaceRows> {
    let held: HashSet<PathBuf> = spaces
        .iter()
        .chain(closed)
        .flat_map(|space| still_held(space, listed))
        .collect();
    let mut first: HashSet<PathBuf> = HashSet::new();
    let mut rows = Vec::with_capacity(spaces.len());
    for space in spaces {
        let own: HashSet<PathBuf> = still_held(space, listed).collect();
        let used: HashSet<PathBuf> = space.sessions.iter().map(|path| spelled(path)).collect();
        let projects = space
            .members
            .iter()
            .map(|root| {
                let is_first = first.insert(spelled(root));
                let shows = |worktree: &Worktree| {
                    let path = spelled(&worktree.path);
                    if worktree.primary {
                        is_first || used.contains(&path)
                    } else {
                        own.contains(&path) || (is_first && !held.contains(&path))
                    }
                };
                let worktrees = (listed.get(*root).into_iter().flatten())
                    .filter(|worktree| shows(worktree))
                    .cloned()
                    .collect();
                Project {
                    root: root.to_path_buf(),
                    name: crate::sessions::spaces::stored::last_component(root),
                    worktrees,
                }
            })
            .collect();
        rows.push(SpaceRows {
            id: space.id.clone(),
            name: space.name.to_owned(),
            projects,
            directory: space.directory.map(Path::to_path_buf),
            orchestrator: None,
        });
    }
    rows
}

/// The worktrees `space` holds that a repository it still has lists, as spelled.
fn still_held<'a>(
    space: &'a SpaceInput<'_>,
    listed: &'a HashMap<PathBuf, Vec<Worktree>>,
) -> impl Iterator<Item = PathBuf> + 'a {
    let holds: HashSet<PathBuf> = space.held.iter().map(|path| spelled(path)).collect();
    (space.members.iter())
        .flat_map(move |root| listed.get(*root).into_iter().flatten())
        .map(|worktree| spelled(&worktree.path))
        .filter(move |path| holds.contains(path))
}

/// `path` as its components spell it, for comparing paths through a set.
fn spelled(path: &Path) -> PathBuf {
    path.components().collect()
}

#[cfg(test)]
mod tests;
