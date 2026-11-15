//! Which worktrees each space of the tree shows (ADR 0136, step 2): those it holds; the main
//! checkout while a session of the space runs there; and in the first space that has the
//! repository, the main checkout and every worktree no space holds. So a worktree shows in one
//! space only, but for a main checkout a session of another space runs in.
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
    /// Where its running sessions run.
    pub running: HashSet<PathBuf>,
}

/// The rows of `spaces`, each repository's worktrees taken from `listed`.
pub fn space_rows(
    spaces: &[SpaceInput<'_>],
    listed: &HashMap<PathBuf, Vec<Worktree>>,
) -> Vec<SpaceRows> {
    let held: HashSet<PathBuf> = spaces
        .iter()
        .flat_map(|space| space.held.iter().map(|path| spelled(path)))
        .collect();
    let mut first: HashSet<PathBuf> = HashSet::new();
    let mut rows = Vec::with_capacity(spaces.len());
    for space in spaces {
        let own: HashSet<PathBuf> = space.held.iter().map(|path| spelled(path)).collect();
        let running: HashSet<PathBuf> = space.running.iter().map(|path| spelled(path)).collect();
        let projects = space
            .members
            .iter()
            .map(|root| {
                let is_first = first.insert(spelled(root));
                let shows = |worktree: &Worktree| {
                    let path = spelled(&worktree.path);
                    if worktree.primary {
                        is_first || running.contains(&path)
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
        });
    }
    rows
}

/// `path` as its components spell it, for comparing paths through a set.
fn spelled(path: &Path) -> PathBuf {
    path.components().collect()
}

#[cfg(test)]
mod tests;
