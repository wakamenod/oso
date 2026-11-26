//! The session the tree's window shows (ADR 0140, "The session shown, as amended"): its rows
//! have their own band, and each time it changes, what hides its row opens.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data in, text out, as the list.

use super::super::{Entry, Fold, Group, Key, List};
use super::{Place, Tree, by_depth, place};
use crate::sessions::spaces::SpaceId;

impl List {
    /// The session the tree's window shows, of `entries`, whose row has its own band, if one
    /// does (ADR 0140). Each time it changes, what hides its row opens: its space, repository
    /// and worktree, the sessions it is nested under, or its group. One folded again by hand
    /// stays folded until it changes again.
    pub fn set_shown(&mut self, shown: Option<Key>, entries: &[Entry]) {
        if self.beat.shown == shown {
            return;
        }
        self.beat.shown = shown;
        let Some(entry) = shown.and_then(|key| entries.iter().find(|e| e.key == key)) else {
            return;
        };
        let (space, folds) = self.tree.hiding(entry, entries);
        if let Some(space) = space {
            self.chosen.insert(space, false);
        }
        for fold in folds {
            self.set.insert(fold, false);
        }
    }
}

impl Tree {
    /// What hides `entry`'s row, of `entries`, wherever the tree places it: the space to open,
    /// and the folds to open, its repository and worktree, the sessions it is nested under, or
    /// its group (ADR 0140).
    pub(super) fn hiding(&self, entry: &Entry, entries: &[Entry]) -> (Option<SpaceId>, Vec<Fold>) {
        let worktrees = by_depth(&self.spaces);
        let mut folds = Vec::new();
        let space = match place(&worktrees, self, entry) {
            Place::Worktree(s, p, w) => {
                let space = &self.spaces[s];
                let project = &space.projects[p];
                folds.push(Fold::Repository(space.id.clone(), project.root.clone()));
                let path = project.worktrees[w].path.clone();
                folds.push(Fold::Worktree(space.id.clone(), path));
                Some(space.id.clone())
            }
            Place::Orchestrator(s) => Some(self.spaces[s].id.clone()),
            Place::Other(dir) => {
                folds.push(Fold::Group(Group::Other));
                folds.push(Fold::Group(Group::OtherDir(dir.to_path_buf())));
                None
            }
            Place::Archived => {
                folds.push(Fold::Group(Group::Archived));
                None
            }
        };
        // The sessions it is nested under, each once, however their parents run.
        let mut at = entry;
        let mut seen = vec![entry.key];
        while let Some(parent) = (at.parent)
            .filter(|key| !seen.contains(key))
            .and_then(|key| entries.iter().find(|e| e.key == key))
        {
            seen.push(parent.key);
            folds.push(Fold::Nested(parent.key));
            at = parent;
        }
        (space, folds)
    }
}
