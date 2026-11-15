//! Issue spaces (ADR 0136): each the workplace of one issue, holding several repositories, the
//! worktrees it made, and its own window tree, swapped whole into the frame as a tab would be,
//! with its zoom, and the session and the file buffer it showed last, for its first entry.
//! What a space keeps across starts is [`Stored`], in the `spaces` file ([`record`]).
//!
//! Must not depend on: `app`, `dispatch` or `config`; it acts on the editor through oso-core's
//! window trees alone, and is given each place's repository: whether a worktree belongs to a
//! repository is ADR 0114's claim, which is checked off this thread.
//!
//! A session belongs to the space its record names. One with none joins by [`Spaces::join`]:
//! the space holding its worktree; else the space in front when the repository is one of its;
//! else the open space entered last that holds the repository; else a new space of that one
//! repository.

pub mod dir;
pub mod edit;
pub mod migrate;
pub mod orchestrator;
pub mod probe;
pub mod record;
pub mod stored;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::complete::Candidate;
use oso_core::editor::{Editor, WindowTree};

pub use stored::{Member, SpaceId, Stored};
use stored::{same, slug, unique};

/// Fewer fresh ids than this asks the worker for more.
const FEW_IDS: usize = 4;

/// One space.
#[derive(Debug)]
pub struct Space {
    pub stored: Stored,
    /// Its window tree while another space's is in the frame.
    tree: Option<WindowTree>,
    /// Its tree from before `space-zoom`, while zoomed.
    zoom: Option<WindowTree>,
    /// Whether it has had a tree: a space never entered gets one from its last buffers.
    entered: bool,
    /// When it was entered last, as a count of entries: the latest is the highest.
    stamp: u64,
    pub last_session: Option<BufferId>,
    pub last_file: Option<BufferId>,
}

impl Space {
    fn new(stored: Stored) -> Self {
        Self {
            stored,
            tree: None,
            zoom: None,
            entered: false,
            stamp: 0,
            last_session: None,
            last_file: None,
        }
    }

    pub fn name(&self) -> &str {
        &self.stored.name
    }

    pub fn id(&self) -> &SpaceId {
        &self.stored.id
    }

    pub fn is_open(&self) -> bool {
        !self.stored.archived
    }
}

/// The open spaces' places by path, from [`Spaces::places`]: keyed by their text as their
/// components spell it, as hashing a path by its components costs several times more.
#[derive(Debug)]
pub struct Places {
    roots: HashMap<OsString, usize>,
}

impl Places {
    /// The space of the deepest worktree or repository above `path`, or at it: the one in
    /// front where two spaces have it.
    pub fn of(&self, path: &Path) -> Option<usize> {
        let spelled: PathBuf = path.components().collect();
        spelled
            .ancestors()
            .find_map(|dir| self.roots.get(dir.as_os_str()).copied())
    }
}

/// The editor has a prompt open, so the frame's tree stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

/// What `space-zoom` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
}

/// Every space oso knows, and the one whose tree is in the frame.
#[derive(Debug, Default)]
pub struct Spaces {
    spaces: Vec<Space>,
    current: Option<usize>,
    /// The tree from before `space-zoom` while no space is current.
    zoom: Option<WindowTree>,
    /// The count of entries, for each space's stamp.
    entries: u64,
    /// Ids read off the UI thread, for spaces made on it.
    fresh: Vec<SpaceId>,
    /// More ids were asked for and have not come.
    asked: bool,
    /// The `spaces` file has been read, or found missing and made from the sessions: until
    /// then no session joins a space, so that none is made twice.
    recalled: bool,
    /// The ids of every space this instance had, for a write to keep another's.
    known: HashSet<SpaceId>,
    /// A space changed since the file was last written.
    unsaved: bool,
    /// The `spaces` file could not be read: a session joins a space in this instance only.
    unreadable: bool,
}

impl Spaces {
    /// Takes the spaces the file held, or were made from the sessions (`migrated`, which are
    /// then kept at once), and `fresh` ids. A space already here stays as it is.
    pub fn recall(&mut self, stored: Vec<Stored>, fresh: Vec<SpaceId>, migrated: bool) {
        for mut space in stored {
            if self.known.insert(space.id.clone()) {
                // Two instances that made a space of one name each keep both: the later gets
                // another name, so that every space can be named.
                if self.named(&space.name).is_some() {
                    space.name = unique(&space.name, &|name| self.named(name).is_some());
                    self.unsaved = true;
                }
                self.spaces.push(Space::new(space));
            }
        }
        self.fresh.extend(fresh);
        self.asked = false;
        self.recalled = true;
        self.unsaved |= migrated;
    }

    /// Notes whether the `spaces` file could be read.
    pub fn set_readable(&mut self, readable: bool) {
        self.unreadable = !readable;
    }

    /// Whether a session's space may be kept in its record: not while the `spaces` file could
    /// not be read, as the spaces made meanwhile would not be kept with it.
    pub fn keeps_joins(&self) -> bool {
        !self.unreadable
    }

    /// Whether the spaces have been recalled, so that sessions may join them.
    pub fn recalled(&self) -> bool {
        self.recalled
    }

    /// Takes ids the worker read, and says whether any came. None coming means none can be
    /// read: no more are asked for, so that the worker and this thread do not ask each other
    /// round and round.
    pub fn give_ids(&mut self, fresh: Vec<SpaceId>) -> bool {
        let came = !fresh.is_empty();
        if came {
            self.fresh.extend(fresh);
            self.asked = false;
        }
        came
    }

    /// Whether to ask the worker for more ids, once: then it is taken as asked.
    pub fn wants_ids(&mut self) -> bool {
        let wants = self.recalled && !self.asked && self.fresh.len() < FEW_IDS;
        self.asked |= wants;
        wants
    }

    /// What the file is to keep, and the ids this instance had, if a space changed since it
    /// was last written; then it counts as written.
    pub fn take_unsaved(&mut self) -> Option<(Vec<Stored>, HashSet<SpaceId>)> {
        if !std::mem::take(&mut self.unsaved) {
            return None;
        }
        let stored = self
            .spaces
            .iter()
            .map(|space| space.stored.clone())
            .collect();
        Some((stored, self.known.clone()))
    }

    /// Every space, in the order they were made or recalled.
    pub fn iter(&self) -> impl Iterator<Item = &Space> {
        self.spaces.iter()
    }

    pub fn get(&self, at: usize) -> Option<&Space> {
        self.spaces.get(at)
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }

    /// The space whose id is `id`.
    pub fn find(&self, id: &SpaceId) -> Option<usize> {
        self.spaces.iter().position(|space| space.stored.id == *id)
    }

    /// The space a `space-switch` answer names.
    pub fn named(&self, name: &str) -> Option<usize> {
        self.spaces
            .iter()
            .position(|space| space.stored.name == name)
    }

    /// The open space that holds the worktree at `path`, which one space does at most; else
    /// the open space with it as a repository's main checkout, as [`Self::with_member`] picks.
    pub fn holding(&self, path: &Path) -> Option<usize> {
        self.open()
            .find(|&at| self.spaces[at].stored.holds_worktree(path))
            .or_else(|| self.with_member(path))
    }

    /// The open space with repository `root`: the one in front, else the one entered last.
    pub fn with_member(&self, root: &Path) -> Option<usize> {
        self.open()
            .filter(|&at| self.spaces[at].stored.has_member(root))
            .max_by_key(|&at| (self.current == Some(at), self.spaces[at].stamp))
    }

    /// The open spaces' worktrees and repositories by their paths, to find the space of many
    /// paths at once, as the sessions of other programs need: one pass over the spaces, then
    /// a lookup per directory above each path.
    pub fn places(&self) -> Places {
        let mut roots: HashMap<OsString, usize> = HashMap::new();
        for at in self.open() {
            let stored = &self.spaces[at].stored;
            let members = stored.members.iter().map(|member| member.root.as_path());
            for root in stored.worktrees.iter().map(PathBuf::as_path).chain(members) {
                let front = self.current == Some(at);
                let spelled: PathBuf = root.components().collect();
                roots
                    .entry(spelled.into_os_string())
                    .and_modify(|kept| {
                        if front {
                            *kept = at;
                        }
                    })
                    .or_insert(at);
            }
        }
        Places { roots }
    }

    /// The open space `path` is in, as [`Places::of`] finds it.
    #[cfg(test)]
    pub fn of_path(&self, path: &Path) -> Option<usize> {
        self.places().of(path)
    }

    /// The space a session that runs at `place` and names no space joins, by the rule in this
    /// module's words. `repo` is the repository `place` is a worktree of, where git listed it,
    /// and the worktree is then held by the space; else the deepest repository of an open space
    /// above `place`, else `place` itself, is taken. `None` until the spaces are
    /// recalled, or while no id is at hand for a new space.
    pub fn join(&mut self, place: &Path, repo: Option<&Path>, now: u64) -> Option<usize> {
        if !self.recalled {
            return None;
        }
        if let Some(at) = self.holding(place) {
            return Some(at);
        }
        let above = || {
            self.open()
                .flat_map(|at| self.spaces[at].stored.members.iter())
                .map(|member| member.root.as_path())
                .filter(|root| place.starts_with(root) && !same(root, place))
                .max_by_key(|root| root.components().count())
                .map(Path::to_path_buf)
        };
        // Only a place git listed is held as a worktree: one found by the path alone may be a
        // directory of the repository, or another repository inside it.
        let worktree = repo.is_some();
        let repo = repo
            .map(Path::to_path_buf)
            .or_else(above)
            .unwrap_or_else(|| place.to_path_buf());
        let at = match self.with_member(&repo) {
            Some(at) => at,
            None => self.make(Member::at(&repo), now)?,
        };
        if worktree {
            self.hold_worktree(at, place, now);
        }
        Some(at)
    }

    /// A new open space of repository `member`, named after it, as of `now`. `None` while no
    /// id is at hand, or with as many spaces as the file keeps.
    pub fn make(&mut self, member: Member, now: u64) -> Option<usize> {
        if self.spaces.len() >= record::MAX_SPACES {
            return None;
        }
        let id = self.fresh.pop()?;
        let name = unique(&slug(&member.name()), &|name| self.named(name).is_some());
        self.known.insert(id.clone());
        self.spaces
            .push(Space::new(Stored::of_repository(id, name, member, now)));
        self.unsaved = true;
        Some(self.spaces.len() - 1)
    }

    /// Records the worktree at `path` in space `at`, and in no other.
    pub fn hold_worktree(&mut self, at: usize, path: &Path, now: u64) {
        for (other, space) in self.spaces.iter_mut().enumerate() {
            let stored = &mut space.stored;
            let had = stored.holds_worktree(path);
            if other == at && !had && stored.worktrees.len() < stored::MAX_WORKTREES {
                stored.worktrees.push(path.to_owned());
            } else if other != at && had {
                stored.worktrees.retain(|worktree| !same(worktree, path));
            } else {
                continue;
            }
            stored.changed = now;
            self.unsaved = true;
        }
    }

    /// Forgets the worktree at `path`, which was removed.
    pub fn forget_worktree(&mut self, path: &Path, now: u64) {
        for space in &mut self.spaces {
            if space.stored.holds_worktree(path) {
                space
                    .stored
                    .worktrees
                    .retain(|worktree| !same(worktree, path));
                space.stored.changed = now;
                self.unsaved = true;
            }
        }
    }

    /// Notes the session and the file buffer space `at` showed last.
    pub fn showed(&mut self, at: usize, session: Option<BufferId>, file: Option<BufferId>) {
        if let Some(space) = self.spaces.get_mut(at) {
            if session.is_some() {
                space.last_session = session;
            }
            if file.is_some() {
                space.last_file = file;
            }
        }
    }

    /// The open spaces' places.
    fn open(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.spaces.len()).filter(|&at| self.spaces[at].is_open())
    }

    /// Space `at` is the one whose tree is in the frame, as it is: oso's first space. A zoom
    /// made before there was a space is the space's.
    pub fn adopt(&mut self, at: usize) {
        if let Some(space) = self.spaces.get_mut(at) {
            space.entered = true;
            self.entries += 1;
            space.stamp = self.entries;
            if self.current.is_none() && space.zoom.is_none() {
                space.zoom = self.zoom.take();
            }
            self.current = Some(at);
        }
    }

    /// Shows space `at`'s tree, keeping the frame's in the space it leaves. A space entered for
    /// the first time shows its last file buffer and its last session side by side, or the one
    /// of them it has; with neither, the frame's tree becomes its own, with its zoom, and the
    /// space left gets one from its last buffers when it is entered again. With no space yet,
    /// the frame's tree becomes `at`'s.
    ///
    /// # Errors
    ///
    /// [`Refused`] while a prompt is open: nothing changed.
    pub fn enter(&mut self, editor: &mut Editor, at: usize) -> Result<(), Refused> {
        let Some(from) = self.current else {
            self.adopt(at);
            return Ok(());
        };
        if from == at || at >= self.spaces.len() {
            return Ok(());
        }
        if editor.prompt().is_some() {
            return Err(Refused);
        }
        let target = &mut self.spaces[at];
        let tree = match target.tree.take() {
            Some(tree) => Some(tree),
            None if target.entered => None,
            None => {
                let shown: Vec<BufferId> = [target.last_file, target.last_session]
                    .into_iter()
                    .flatten()
                    .filter(|&id| editor.buffer(id).is_some())
                    .collect();
                (!shown.is_empty()).then(|| editor.window_tree_showing(&shown))
            }
        };
        match tree {
            Some(tree) => match editor.swap_window_tree(tree) {
                Ok(old) => self.spaces[from].tree = Some(old),
                Err(tree) => {
                    let target = &mut self.spaces[at];
                    if target.entered {
                        target.tree = Some(tree);
                    }
                    return Err(Refused);
                }
            },
            None => {
                let from = &mut self.spaces[from];
                from.entered = false;
                let zoom = from.zoom.take();
                self.spaces[at].zoom = zoom;
            }
        }
        self.entries += 1;
        let space = &mut self.spaces[at];
        space.entered = true;
        space.stamp = self.entries;
        self.current = Some(at);
        Ok(())
    }

    /// `space-zoom`: the tree's selected window alone, and back, with that window's buffer and
    /// places put in the tree's window it was zoomed from. With the side window selected, the
    /// tree's window is selected first.
    ///
    /// # Errors
    ///
    /// [`Refused`] while a prompt is open.
    pub fn zoom(&mut self, editor: &mut Editor) -> Result<Zoom, Refused> {
        if editor.prompt().is_some() {
            return Err(Refused);
        }
        let side = editor.side_window();
        if side == Some(editor.window().id()) {
            editor.other_window(1);
        }
        let slot = match self.current {
            Some(at) => &mut self.spaces[at].zoom,
            None => &mut self.zoom,
        };
        let Some(saved) = slot.take() else {
            let lone = editor.lone_window_tree();
            let saved = editor.swap_window_tree(lone).map_err(|_| Refused)?;
            *slot = Some(saved);
            return Ok(Zoom::In);
        };
        let window = editor.window();
        let (buffer, selections, scroll) =
            (window.buffer(), window.selections.clone(), window.scroll());
        if let Err(saved) = editor.swap_window_tree(saved) {
            *slot = Some(saved);
            return Err(Refused);
        }
        let back = editor.window().id();
        editor.show_in(back, buffer);
        editor.restore_positions_in(back, selections, scroll);
        Ok(Zoom::Out)
    }

    /// The candidates of `space-switch`: each open space's name, the one in front last, with
    /// its title, else its repositories' names, as the annotation.
    pub fn candidates(&self) -> Arc<[Candidate]> {
        let front = self
            .current
            .and_then(|at| self.spaces.get(at))
            .map(Space::id);
        let mut order: Vec<&Space> = self.spaces.iter().filter(|s| s.is_open()).collect();
        order.sort_by(|a, b| {
            let last = |space: &Space| Some(space.id()) == front;
            (last(a), &a.stored.name).cmp(&(last(b), &b.stored.name))
        });
        order
            .into_iter()
            .map(|space| {
                let stored = &space.stored;
                // Directory names are any text: cleaned as the list's untrusted fields are.
                let about = if stored.title.is_empty() {
                    let names: Vec<String> = stored.members.iter().map(Member::name).collect();
                    crate::sessions::recap::clean(&names.join(", "))
                } else {
                    crate::sessions::recap::clean(&stored.title)
                };
                Candidate::new(stored.name.as_str(), "", about)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
