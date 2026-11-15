//! What the person's space commands change (ADR 0136, step 1b): a new space for an issue, its
//! repositories, its title and name, its directory once made, and archiving and reopening it.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain changes to the spaces, with the
//! frame's tree changed only through [`Spaces::enter`], as a switch does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::complete::Candidate;
use oso_core::editor::Editor;

use super::stored::{MAX_MEMBERS, Member, Stored, clean_title, same, slug, unique};
use super::{Refused, Space, Spaces, record};
use crate::sessions::state::State;

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

    /// Notes the device and inode of repository `root` of space `at`, looked at on a worker,
    /// when none is kept: one kept is never replaced, so that a repository put in another's
    /// place is told apart when it is checked.
    pub fn set_identity(&mut self, at: usize, root: &Path, identity: Option<(u64, u64)>) {
        let Some(space) = self.spaces.get_mut(at) else {
            return;
        };
        let found = space
            .stored
            .members
            .iter_mut()
            .find(|m| same(&m.root, root));
        if let Some(member) = found.filter(|member| member.identity.is_none()) {
            member.identity = identity;
            self.unsaved = true;
        }
    }

    /// Notes the directory made for space `at`, and the name it was made under, which the
    /// space then takes when no other space has it and it was not renamed since it was asked
    /// for under `asked` (ADR 0136, step 1b, P3).
    pub fn set_directory(
        &mut self,
        at: usize,
        path: PathBuf,
        identity: Option<(u64, u64)>,
        name: &str,
        asked: &str,
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
        if !taken && slug(name) == name && stored.name == asked {
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
                    // The frame keeps the tree, which is no space's now: entered again, the
                    // space shows its last buffers.
                    self.current = None;
                    self.zoom = self.spaces[at].zoom.take();
                    self.spaces[at].entered = false;
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

    /// Notes the orchestrator `id` of space `at`, the one before it kept as an earlier one,
    /// at most [`MAX_EARLIER`](super::stored::MAX_EARLIER) of them.
    /// Space `at`'s orchestrator replaced: kept as an earlier one, and no session is its
    /// orchestrator until its successor starts (ADR 0136, step 5).
    pub fn retire_orchestrator(&mut self, at: usize, now: u64) {
        let Some(space) = self.spaces.get_mut(at) else {
            return;
        };
        let stored = &mut space.stored;
        if let Some(before) = stored.orchestrator.take() {
            stored.earlier.push(before);
            let over = (stored.earlier.len()).saturating_sub(super::stored::MAX_EARLIER);
            stored.earlier.drain(..over);
            stored.changed = now;
            self.unsaved = true;
        }
    }

    pub fn set_orchestrator(
        &mut self,
        at: usize,
        id: crate::sessions::identity::SessionId,
        now: u64,
    ) {
        let Some(space) = self.spaces.get_mut(at) else {
            return;
        };
        let stored = &mut space.stored;
        if stored.orchestrator.as_ref() == Some(&id) {
            return;
        }
        if let Some(before) = stored.orchestrator.replace(id) {
            stored.earlier.push(before);
            let over = stored
                .earlier
                .len()
                .saturating_sub(super::stored::MAX_EARLIER);
            stored.earlier.drain(..over);
        }
        stored.changed = now;
        self.unsaved = true;
    }

    /// Space `at`'s repositories by their names in it, as [`Self::member_candidates`] lists the
    /// space in front's.
    pub fn member_names(&self, at: usize) -> Vec<(String, PathBuf)> {
        self.spaces
            .get(at)
            .map_or_else(Vec::new, |space| names_of(&space.stored))
    }

    /// The candidates of `space-remove-repo`: the repositories of the space in front, by
    /// name, two of one name told apart with `<2>` as buffers are, with their roots.
    pub fn member_candidates(&self) -> Arc<[Candidate]> {
        self.front_member_names()
            .into_iter()
            .map(|(name, root)| {
                let shown = crate::sessions::recap::clean(&root.display().to_string());
                Candidate::new(name.as_str(), "", shown)
            })
            .collect()
    }

    /// The repository of the space in front named `name`, as `space-remove-repo` lists it.
    pub fn member_named(&self, name: &str) -> Option<PathBuf> {
        self.front_member_names()
            .into_iter()
            .find(|(named, _)| named == name)
            .map(|(_, root)| root)
    }

    /// The space in front's repositories by their names in it: each root's last component,
    /// cleaned, `<2>` and on after the first of a name.
    fn front_member_names(&self) -> Vec<(String, PathBuf)> {
        self.current
            .map_or_else(Vec::new, |at| self.member_names(at))
    }

    /// Every open space's repositories, each once, in the spaces' order.
    pub fn members(&self) -> Vec<PathBuf> {
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut roots: Vec<PathBuf> = Vec::new();
        for space in self.spaces.iter().filter(|space| space.is_open()) {
            for member in &space.stored.members {
                if seen.insert(member.root.components().collect()) {
                    roots.push(member.root.clone());
                }
            }
        }
        roots
    }
}

/// `stored`'s repositories by their names in it: each root's last component, cleaned, `<2>`
/// and on after the first of a name.
fn names_of(stored: &Stored) -> Vec<(String, PathBuf)> {
    let mut named: Vec<(String, PathBuf)> = Vec::new();
    for member in &stored.members {
        let base = crate::sessions::recap::clean(&member.name());
        let mut name = base.clone();
        let mut n = 2;
        while named.iter().any(|(taken, _)| *taken == name) {
            name = format!("{base}<{n}>");
            n += 1;
        }
        named.push((name, member.root.clone()));
    }
    named
}

/// Which of a space's running sessions closing it stops, as `(key, name, state)`: all of
/// them when each is idle, done or ready for review; else the name of the first that works or
/// needs input, and none is stopped.
///
/// # Errors
///
/// The name of a session that keeps the space open.
pub fn sessions_to_stop<K>(sessions: Vec<(K, String, State)>) -> Result<Vec<K>, String> {
    let mut idle = Vec::with_capacity(sessions.len());
    for (key, name, state) in sessions {
        match state {
            State::Idle | State::Done | State::ReadyForReview => idle.push(key),
            _ => return Err(name),
        }
    }
    Ok(idle)
}

#[cfg(test)]
mod tests;
