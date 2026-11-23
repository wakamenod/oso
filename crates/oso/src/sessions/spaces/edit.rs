//! What the person's space commands change (ADR 0136, step 1b): a new space for an issue, its
//! repositories, its display name, its directory once made, and archiving and reopening it.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain changes to the spaces, with the
//! frame's tree changed only through [`Spaces::enter`], as a switch does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::complete::Candidate;
use oso_core::editor::Editor;

use super::orchestrator::is_earlier_name;
use super::stored::{
    MAX_MEMBERS, Member, Stored, clean_description, clean_display, same, slug, unique,
    unique_display,
};
use super::{Refused, Space, Spaces, record};
use crate::sessions::state::State;

/// What a new space is called and what it is for (ADR 0159): its description, and the display
/// name and the slug a summary of it, or the fallback, gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Naming {
    pub description: String,
    pub display: String,
    /// The base of its slug, made a slug and unique when the space is made.
    pub slug: String,
}

/// Why a repository was not added to a space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotAdded {
    /// The space has it already.
    Already,
    /// The space has as many as it may.
    Full,
}

impl Spaces {
    /// A new open space of repository `member`, named as `naming` says, as of `now`; it is
    /// not entered. Its slug is unlike every space's, every earlier orchestrator's
    /// `<space>-was-<n>`, and every name `taken` holds, a session's, so that its orchestrator
    /// answers to its name alone (ADR 0159); its display name is unlike every space's. `None`
    /// while no id is at hand, or with as many spaces as the file keeps.
    pub fn new_space(
        &mut self,
        naming: &Naming,
        member: Member,
        now: u64,
        taken: &dyn Fn(&str) -> bool,
    ) -> Option<usize> {
        let at = self.make(member, now)?;
        let base = slug(&naming.slug);
        let name = unique(&base, &|name| {
            taken(name) || self.slug_taken(name, Some(at))
        });
        let display = self.display_for(at, &naming.display, &name);
        let stored = &mut self.spaces[at].stored;
        stored.description = clean_description(&naming.description);
        stored.name = name;
        stored.display = display.unwrap_or_default();
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
    /// space then takes as its slug when no other space, nor a name `sessions` holds, has it,
    /// and it was asked for under `asked` (ADR 0136, step 1b, P3). From then on its slug never
    /// changes (ADR 0159).
    pub fn set_directory(
        &mut self,
        at: usize,
        path: PathBuf,
        identity: Option<(u64, u64)>,
        (name, asked): (&str, &str),
        sessions: &dyn Fn(&str) -> bool,
        now: u64,
    ) {
        let taken = sessions(name) || self.slug_taken(name, Some(at));
        let Some(space) = self.spaces.get_mut(at) else {
            return;
        };
        let stored = &mut space.stored;
        // A space has one directory: a later job's, made elsewhere, changes neither it nor its
        // slug, which its orchestrator may answer to already.
        if (stored.directory.as_deref()).is_some_and(|kept| !same(kept, &path)) {
            log::warn!("a second directory was made for a space; it keeps its first");
            return;
        }
        stored.directory = Some(path);
        stored.directory_identity = identity;
        if !taken && slug(name) == name && stored.name == asked {
            stored.name = name.to_owned();
        }
        stored.changed = now;
        self.unsaved = true;
    }

    /// Gives space `at` the display name `display`, cleaned; its slug, directory and
    /// orchestrator's name stay as they are (ADR 0159).
    ///
    /// # Errors
    ///
    /// Why it is refused, for the echo area: nothing is left of it, or another space is
    /// called so.
    pub fn rename(&mut self, at: usize, display: &str, now: u64) -> Result<String, String> {
        let display = clean_display(display);
        if display.is_empty() {
            return Err("A space's name needs a character oso shows".to_owned());
        }
        if at >= self.spaces.len() {
            return Err("No such space".to_owned());
        }
        if self.label_taken(&display, Some(at)) {
            return Err(format!("Another space is called {display}"));
        }
        let stored = &mut self.spaces[at].stored;
        stored.display = if display == stored.name {
            String::new()
        } else {
            display.clone()
        };
        stored.changed = now;
        self.unsaved = true;
        Ok(display)
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
                    self.kept = std::mem::take(&mut self.spaces[at].kept);
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

    /// Opens archived space `at` again; it is not entered. A slug another space took meanwhile
    /// is told apart while the space has no directory; once it has one, its slug never changes
    /// (ADR 0159).
    pub fn reopen(&mut self, at: usize, now: u64) {
        let Some(space) = self.spaces.get(at).filter(|space| !space.is_open()) else {
            return;
        };
        let name = if space.stored.directory.is_some() {
            space.stored.name.clone()
        } else {
            unique(&space.stored.name, &|name| self.slug_taken(name, Some(at)))
        };
        let stored = &mut self.spaces[at].stored;
        stored.name = name;
        stored.archived = false;
        stored.changed = now;
        self.unsaved = true;
    }

    /// Whether `name` is the slug of a space other than `at`, or a name one of its earlier
    /// orchestrators keeps, `<slug>-was-<n>`, or a slug whose earlier orchestrators would take
    /// another space's slug, so that no two sessions answer to one name.
    pub fn slug_taken(&self, name: &str, at: Option<usize>) -> bool {
        // Only a name with `-was-` in it can be an earlier orchestrator's.
        let earlier = name.to_ascii_lowercase().contains("-was-");
        (self.spaces.iter().enumerate()).any(|(other, space)| {
            let slug = &space.stored.name;
            (Some(other) != at && slug == name)
                || (earlier && is_earlier_name(slug, name))
                // A slug another space has may be `name`'s earlier orchestrator's.
                || (slug.contains("-was-") && is_earlier_name(name, slug))
        })
    }

    /// Gives space `at` the names a summary of its description gave (ADR 0159, step 2): `slug`
    /// as its slug, made unique as a new space's is, while it has no directory; `display` as
    /// its display name, made unique, if given. Returns the space's slug and what the person
    /// sees it called now.
    pub fn take_names(
        &mut self,
        at: usize,
        (slug, display): (Option<&str>, Option<&str>),
        taken: &dyn Fn(&str) -> bool,
        now: u64,
    ) -> Option<(String, String)> {
        let space = self.spaces.get(at)?;
        let mut name = space.stored.name.clone();
        if let Some(base) = slug.filter(|_| space.stored.directory.is_none()) {
            name = unique(base, &|candidate| {
                taken(candidate) || self.slug_taken(candidate, Some(at))
            });
        }
        let display = display.and_then(|display| self.display_for(at, display, &name));
        let stored = &mut self.spaces[at].stored;
        if name != stored.name || display.is_some() {
            stored.name = name;
            if let Some(display) = display {
                stored.display = display;
            }
            stored.changed = now;
            self.unsaved = true;
        }
        Some((stored.name.clone(), stored.label().to_owned()))
    }

    /// What space `at`, whose slug is `slug`, keeps as its display name for `raw`, cleaned:
    /// empty where it reads as its slug, else unique among the spaces; `None` where nothing of
    /// it is left.
    fn display_for(&self, at: usize, raw: &str, slug: &str) -> Option<String> {
        let display = clean_display(raw);
        if display.is_empty() {
            return None;
        }
        if display == slug {
            return Some(String::new());
        }
        Some(unique_display(&display, &|label| {
            label == slug || self.label_taken(label, Some(at))
        }))
    }

    /// Gives space `at`, which has no directory yet, a slug no session answers to, as `taken`
    /// says, when its own is one: a space made by a session's joining is named after its
    /// repository, as that session often is, and its orchestrator would answer to that name
    /// too (ADR 0159). Whether it changed.
    pub fn free_slug(&mut self, at: usize, taken: &dyn Fn(&str) -> bool, now: u64) -> bool {
        let Some(space) = self.spaces.get(at) else {
            return false;
        };
        if space.stored.directory.is_some() || !taken(&space.stored.name) {
            return false;
        }
        let name = unique(&space.stored.name, &|name| {
            taken(name) || self.slug_taken(name, Some(at))
        });
        let stored = &mut self.spaces[at].stored;
        stored.name = name;
        stored.changed = now;
        self.unsaved = true;
        true
    }

    /// Space `at`'s own stored data.
    pub fn stored(&self, at: usize) -> Option<&Stored> {
        self.spaces.get(at).map(|space| &space.stored)
    }

    /// The archived space the person sees called `name`, else of that slug.
    pub fn archived_named(&self, name: &str) -> Option<usize> {
        let archived = (0..self.spaces.len()).filter(|&at| !self.spaces[at].is_open());
        (archived.clone())
            .find(|&at| self.spaces[at].label() == name)
            .or_else(|| {
                archived
                    .clone()
                    .find(|&at| self.spaces[at].stored.name == name)
            })
    }

    /// Whether as many spaces as the file keeps are made.
    pub fn full(&self) -> bool {
        self.spaces.len() >= record::MAX_SPACES
    }

    /// The candidates of `space-reopen`: each archived space as the person sees it called,
    /// as `space-switch` lists the open ones.
    pub fn archived_candidates(&self) -> Arc<[Candidate]> {
        let mut order: Vec<&Space> = self.spaces.iter().filter(|s| !s.is_open()).collect();
        order.sort_by_key(|space| std::cmp::Reverse(space.stored.changed));
        order.into_iter().map(super::candidate).collect()
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

    /// Space `at`'s orchestrator `id` forgotten, as it ended before it was given a message, so
    /// Claude Code keeps no conversation of it: the one before it, if any, is its orchestrator
    /// again (ADR 0176, the author's Q-O8 (a)). Nothing if `id` is not its orchestrator.
    pub fn forget_orchestrator(
        &mut self,
        at: usize,
        id: &crate::sessions::identity::SessionId,
        now: u64,
    ) {
        let Some(space) = self.spaces.get_mut(at) else {
            return;
        };
        let stored = &mut space.stored;
        if stored.orchestrator.as_ref() != Some(id) {
            return;
        }
        stored.orchestrator = stored.earlier.pop();
        stored.changed = now;
        self.unsaved = true;
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
