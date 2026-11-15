//! What the person's space commands change (ADR 0136, step 1b): a new space for an issue, its
//! repositories, its title and name, its directory once made, and archiving and reopening it.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain changes to the spaces, with the
//! frame's tree changed only through [`Spaces::enter`], as a switch does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::complete::Candidate;
use oso_core::editor::Editor;

use super::stored::{self, MAX_MEMBERS, Member, Stored, clean_title, same, slug, unique};
use super::{Refused, Space, Spaces, record};

/// Why a repository was not added to a space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotAdded {
    /// The space has it already.
    Already,
    /// The space has as many as it may.
    Full,
}

impl Spaces {
    /// A new open space for the issue `title`, cleaned, of repository `member`, named after
    /// the title, as of `now`; it is not entered. `None` while no id is at hand, or with as
    /// many spaces as the file keeps.
    pub fn new_space(&mut self, title: &str, member: Member, now: u64) -> Option<usize> {
        let at = self.make(member, now)?;
        let title = clean_title(title);
        let base = if title.is_empty() {
            slug(&self.spaces[at].stored.members[0].name())
        } else {
            slug(&title)
        };
        let name = unique(&base, &|name| {
            self.spaces
                .iter()
                .enumerate()
                .any(|(other, space)| other != at && space.stored.name == name)
        });
        let stored = &mut self.spaces[at].stored;
        stored.title = title;
        stored.name = name;
        Some(at)
    }

    /// Adds repository `member` to space `at`.
    ///
    /// # Errors
    ///
    /// [`NotAdded`] when the space has it, or is full.
    pub fn add_member(&mut self, at: usize, member: Member, now: u64) -> Result<(), NotAdded> {
        let Some(space) = self.spaces.get_mut(at) else {
            return Ok(());
        };
        let stored = &mut space.stored;
        if stored.has_member(&member.root) {
            return Err(NotAdded::Already);
        }
        if stored.members.len() >= MAX_MEMBERS {
            return Err(NotAdded::Full);
        }
        stored.members.push(member);
        stored.changed = now;
        self.unsaved = true;
        Ok(())
    }

    /// Takes repository `root` out of space `at`; its worktrees stay held. Whether it had it.
    pub fn remove_member(&mut self, at: usize, root: &Path, now: u64) -> bool {
        let Some(space) = self.spaces.get_mut(at) else {
            return false;
        };
        let stored = &mut space.stored;
        let before = stored.members.len();
        stored.members.retain(|member| !same(&member.root, root));
        let removed = stored.members.len() != before;
        if removed {
            stored.changed = now;
            self.unsaved = true;
        }
        removed
    }

    /// Notes the device and inode of repository `root` of space `at`, looked at on a worker.
    pub fn set_identity(&mut self, at: usize, root: &Path, identity: Option<(u64, u64)>) {
        let Some(space) = self.spaces.get_mut(at) else {
            return;
        };
        let found = space
            .stored
            .members
            .iter_mut()
            .find(|m| same(&m.root, root));
        if let Some(member) = found.filter(|member| member.identity != identity) {
            member.identity = identity;
            self.unsaved = true;
        }
    }

    /// Notes the directory made for space `at`, and the name it was made under, which the
    /// space then takes when no other space has it (ADR 0136, step 1b, P3).
    pub fn set_directory(
        &mut self,
        at: usize,
        path: PathBuf,
        identity: Option<(u64, u64)>,
        name: &str,
        now: u64,
    ) {
        let taken = self
            .spaces
            .iter()
            .enumerate()
            .any(|(other, space)| other != at && space.stored.name == name);
        let Some(space) = self.spaces.get_mut(at) else {
            return;
        };
        let stored = &mut space.stored;
        stored.directory = Some(path);
        stored.directory_identity = identity;
        if !taken && slug(name) == name {
            stored.name = name.to_owned();
        }
        stored.changed = now;
        self.unsaved = true;
    }

    /// Gives space `at` the title `title`, cleaned, and a name made from it.
    pub fn rename(&mut self, at: usize, title: &str, now: u64) {
        let title = clean_title(title);
        if title.is_empty() || at >= self.spaces.len() {
            return;
        }
        let name = unique(&slug(&title), &|name| {
            self.spaces
                .iter()
                .enumerate()
                .any(|(other, space)| other != at && space.stored.name == name)
        });
        let stored = &mut self.spaces[at].stored;
        stored.title = title;
        stored.name = name;
        stored.changed = now;
        self.unsaved = true;
    }

    /// Archives space `at`. If its tree is in the frame, the other open space entered last is
    /// entered first, else none is current and the frame keeps its tree (P4).
    ///
    /// # Errors
    ///
    /// [`Refused`] while a prompt is open and the space is in the frame: nothing changed.
    pub fn archive(&mut self, editor: &mut Editor, at: usize, now: u64) -> Result<(), Refused> {
        if at >= self.spaces.len() {
            return Ok(());
        }
        if self.current == Some(at) {
            let next = (0..self.spaces.len())
                .filter(|&other| other != at && self.spaces[other].is_open())
                .max_by_key(|&other| self.spaces[other].stamp);
            match next {
                Some(to) => self.enter(editor, to)?,
                None => {
                    self.current = None;
                    self.zoom = self.spaces[at].zoom.take();
                }
            }
        }
        let stored = &mut self.spaces[at].stored;
        stored.archived = true;
        stored.changed = now;
        self.unsaved = true;
        Ok(())
    }

    /// Opens archived space `at` again; it is not entered. A name another open space took
    /// meanwhile is told apart.
    pub fn reopen(&mut self, at: usize, now: u64) {
        let Some(space) = self.spaces.get(at).filter(|space| !space.is_open()) else {
            return;
        };
        let name = unique(&space.stored.name, &|name| {
            self.spaces
                .iter()
                .enumerate()
                .any(|(other, space)| other != at && space.is_open() && space.stored.name == name)
        });
        let stored = &mut self.spaces[at].stored;
        stored.name = name;
        stored.archived = false;
        stored.changed = now;
        self.unsaved = true;
    }

    /// Space `at`'s own stored data.
    pub fn stored(&self, at: usize) -> Option<&Stored> {
        self.spaces.get(at).map(|space| &space.stored)
    }

    /// The archived space named `name`.
    pub fn archived_named(&self, name: &str) -> Option<usize> {
        self.spaces
            .iter()
            .position(|space| !space.is_open() && space.stored.name == name)
    }

    /// Whether as many spaces as the file keeps are made.
    pub fn full(&self) -> bool {
        self.spaces.len() >= record::MAX_SPACES
    }

    /// The candidates of `space-reopen`: each archived space's name, with its title.
    pub fn archived_candidates(&self) -> Arc<[Candidate]> {
        let mut order: Vec<&Space> = self.spaces.iter().filter(|s| !s.is_open()).collect();
        order.sort_by_key(|space| std::cmp::Reverse(space.stored.changed));
        order
            .into_iter()
            .map(|space| {
                let about = crate::sessions::recap::clean(&space.stored.title);
                Candidate::new(space.stored.name.as_str(), "", about)
            })
            .collect()
    }

    /// The candidates of `space-remove-repo`: the repositories of the space in front, by
    /// name, with their roots.
    pub fn member_candidates(&self) -> Arc<[Candidate]> {
        let Some(space) = self.current.and_then(|at| self.spaces.get(at)) else {
            return Arc::from([]);
        };
        space
            .stored
            .members
            .iter()
            .map(|member| {
                let name = crate::sessions::recap::clean(&member.name());
                let root = crate::sessions::recap::clean(&member.root.display().to_string());
                Candidate::new(name.as_str(), "", root)
            })
            .collect()
    }

    /// The repository of the space in front named `name`, as `space-remove-repo` lists it.
    pub fn member_named(&self, name: &str) -> Option<PathBuf> {
        let space = self.current.and_then(|at| self.spaces.get(at))?;
        let named = |member: &&Member| crate::sessions::recap::clean(&member.name()) == name;
        space
            .stored
            .members
            .iter()
            .find(named)
            .map(|member| member.root.clone())
    }

    /// Every open space's repositories, each once, in the spaces' order.
    pub fn members(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = Vec::new();
        for space in self.spaces.iter().filter(|space| space.is_open()) {
            for member in &space.stored.members {
                if !roots.iter().any(|root| same(root, &member.root)) {
                    roots.push(member.root.clone());
                }
            }
        }
        roots
    }
}

/// `stored`'s limit on repositories, for what is said when a space is full.
pub const MOST_MEMBERS: usize = stored::MAX_MEMBERS;

#[cfg(test)]
mod tests;
