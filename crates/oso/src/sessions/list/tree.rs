//! The tree of `*Sessions*` (ADR 0131, step 2; ADR 0136, step 2): each open issue space, its
//! repositories, the worktrees it shows of each, the main checkout first and then the newest
//! made, and the sessions in each, the most urgent first. A space of one repository shows its
//! worktrees right under its own row. An external session in a worktree of the tree is listed
//! under it, quiet; every other one goes in `Other projects`, by the repository or directory it
//! runs in. A worktree with no session and no change for a week goes in its repository's quiet
//! row; closed spaces go in `Archived spaces`, their sessions with the archived ones. A
//! worktree's row never moves when a state changes, though a space unfolds when one of its
//! sessions comes to work or need input (ADR 0136, Q4): urgency shows in the marks, and the
//! roll-ups of a worktree, a repository and a space.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data in, text out, as the list.

mod choose;
mod space;

pub use choose::{SpaceInput, space_rows};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use oso_core::syntax::Highlight;

use super::{
    Entry, Fold, Group, Key, Line, List, PLACE_FROM, Rendered, Row, SPINNER, age, age_of, cells,
    cut, cut_middle, face, mark, order, pad, rank,
};
use crate::sessions::recap::clean;
use crate::sessions::spaces::SpaceId;
use crate::sessions::state::State;

/// How long a worktree with no session stays unchanged before it goes in the quiet row.
pub const QUIET_AFTER: Duration = Duration::from_secs(7 * 86_400);

/// The mark of a worktree with no session: one of the row marks, so that `n` and `p` stop on it.
pub const NO_SESSION: char = '·';

/// The indents of the tree's rows, by depth: a space's, a repository's or a one-repository
/// space's worktree's, a worktree's of a space of several, and their sessions'. A row two
/// cells in from the fold row above it is a heading for `M-n` (`sessions-next-heading`); a
/// session never is.
const INDENT: [&str; 5] = ["", "  ", "    ", "      ", "        "];

/// The indent of an archived session's row, and of one in `Other projects`: never two cells in
/// from a fold row, so that `M-n` passes them.
const SESSION: &str = "    ";
const OTHER_SESSION: &str = "      ";

/// A project of the tree: a repository with a space in this oso.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub worktrees: Vec<Worktree>,
}

/// A worktree of a project: as git lists it in a trusted project, else a directory where a
/// session of the project runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// Its branch, from git, untrusted text.
    pub branch: Option<String>,
    /// The main checkout.
    pub primary: bool,
    /// When it was made: the newest come first after the main checkout.
    pub made: Option<SystemTime>,
    /// When git last changed in it, for the quiet row.
    pub active: Option<SystemTime>,
}

/// An open space of the tree (ADR 0136): its repositories, each with the worktrees the space
/// shows of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceRows {
    pub id: SpaceId,
    pub name: String,
    pub projects: Vec<Project>,
    /// Its own directory, where its orchestrator runs (ADR 0136, step 3).
    pub directory: Option<PathBuf>,
    /// Its orchestrator's session, which the record names: the others in its directory are
    /// earlier ones (step 5).
    pub orchestrator: Option<super::Key>,
}

/// A closed space, listed in `Archived spaces`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosedSpace {
    pub id: SpaceId,
    pub name: String,
    pub title: String,
}

/// What the tree shows besides the sessions.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub spaces: Vec<SpaceRows>,
    pub closed: Vec<ClosedSpace>,
    /// The space in front.
    pub front_space: Option<SpaceId>,
    /// The worktree, or project root, the space in front showed last.
    pub front: Option<PathBuf>,
    /// The wall-clock time the render's `now` stands for, for worktrees' activity.
    pub wall: Option<SystemTime>,
}

/// Where a session goes in the tree.
enum Place<'e> {
    /// Under worktree `.2` of repository `.1` of space `.0`.
    Worktree(usize, usize, usize),
    /// Space `.0`'s orchestrator, in the space's own directory.
    Orchestrator(usize),
    /// In `Other projects`, under this directory.
    Other(&'e Path),
    Archived,
}

/// A space's sessions, by repository and worktree, as placed.
type Placed<'e> = Vec<Vec<Vec<&'e Entry>>>;

impl List {
    /// The tree of `entries` for a window `width` cells wide, ages as of `now`.
    pub(super) fn render_tree(&self, entries: &[Entry], width: usize, now: Instant) -> Rendered {
        let mut out = Rendered::default();
        let spaces = &self.tree.spaces;
        let mut orchestrators: Vec<Vec<&Entry>> = vec![Vec::new(); spaces.len()];
        let mut placed: Vec<Placed<'_>> = spaces
            .iter()
            .map(|space| {
                (space.projects.iter())
                    .map(|project| vec![Vec::new(); project.worktrees.len()])
                    .collect()
            })
            .collect();
        let mut other: Vec<(&Path, Vec<&Entry>)> = Vec::new();
        // Each directory's place in `other`, by its bytes: cheaper than comparing paths.
        let mut other_at: HashMap<&[u8], usize> = HashMap::new();
        let mut archived = Vec::new();
        let worktrees = by_depth(spaces);
        for entry in entries {
            match place(&worktrees, &self.tree, entry) {
                Place::Worktree(s, p, w) => placed[s][p][w].push(entry),
                Place::Orchestrator(s) => orchestrators[s].push(entry),
                Place::Other(dir) => {
                    let at = *other_at
                        .entry(dir.as_os_str().as_encoded_bytes())
                        .or_insert_with(|| {
                            other.push((dir, Vec::new()));
                            other.len() - 1
                        });
                    other[at].1.push(entry);
                }
                Place::Archived => archived.push(entry),
            }
        }
        for ((space, placed), orchestrator) in spaces.iter().zip(&placed).zip(&orchestrators) {
            self.space(&mut out, (space, orchestrator), placed, width, now);
        }
        self.other(&mut out, &mut other, width, now);
        self.archived(&mut out, &archived, width, now);
        self.closed(&mut out, width);
        out
    }

    /// A worktree's row, then its sessions unless it is folded: by default, while none of
    /// oso's runs in it.
    fn worktree(
        &self,
        out: &mut Rendered,
        (space, worktree, depth): (&SpaceRows, &Worktree, usize),
        sessions: &[&Entry],
        width: usize,
        now: Instant,
    ) {
        let indent = INDENT[depth];
        let line = Line::Worktree(space.id.clone(), worktree.path.clone());
        let hosted: Vec<&Entry> = sessions
            .iter()
            .copied()
            .filter(|e| matches!(e.key, Key::Hosted(_)))
            .collect();
        let running = hosted.iter().any(|e| e.state != State::Stopped);
        let fold = Fold::Worktree(space.id.clone(), worktree.path.clone());
        let folded = self.folded_as(&fold, !running);
        let roll = worst(sessions);
        // It spins while its roll-up is a working session of oso's (Q13).
        let spin = self.spin.filter(|_| {
            roll.is_some_and(|e| e.state == State::Working)
                && worst(&hosted).is_some_and(|e| e.state == State::Working)
        });
        let (mark, mark_face) = match (spin, roll) {
            (Some(frame), _) => (SPINNER[frame], Highlight::SessionWorking),
            (None, Some(top)) if matches!(top.key, Key::Hosted(_)) => {
                (mark(top.state), face(top.state))
            }
            (None, Some(top)) => (mark(top.state), Highlight::SessionQuiet),
            (None, None) => (NO_SESSION, Highlight::SessionQuiet),
        };
        let (name, tag) = names(worktree);
        let count = if folded && !sessions.is_empty() {
            format!(" ({})", sessions.len())
        } else {
            String::new()
        };
        let head = format!("{indent}{mark} ");
        let age = super::shown_age(
            self.worktree_age(worktree, sessions, now),
            width,
            cells(&head) + cells(&count),
            &name,
        );
        let room = width.saturating_sub(cells(&head) + cells(&age) + cells(&count) + 1);
        let name = cut_middle(&name, room);
        let mut text = format!("{head}{name}");
        let mut spans = vec![(indent.len()..head.len() - 1, mark_face)];
        let front = self.tree.front_space.as_ref() == Some(&space.id)
            && self.tree.front.as_deref() == Some(worktree.path.as_path());
        if front {
            spans.push((head.len()..text.len(), Highlight::SessionFront));
        }
        let left = room.saturating_sub(cells(&name) + 2);
        if let Some(tag) = tag.filter(|_| left >= PLACE_FROM) {
            text.push_str("  ");
            let at = text.len();
            text.push_str(&cut_middle(super::unmarked(&tag), left));
            spans.push((at..text.len(), Highlight::SessionQuiet));
        }
        let at = text.len();
        text.push_str(&count);
        spans.push((at..text.len(), Highlight::SessionQuiet));
        let text = pad(&text, width.saturating_sub(cells(&age))) + &age;
        spans.push((text.len() - age.len()..text.len(), Highlight::SessionQuiet));
        if spin.is_some() {
            out.spinning.push(out.lines.len());
        }
        out.push_drawn(&cut(&text, width), line, &spans);
        if folded {
            return;
        }
        for (top, nested) in families(sessions) {
            let waiting = nested
                .iter()
                .filter(|e| matches!(e.state, State::NeedsInput(_)))
                .count();
            self.session(out, top, INDENT[depth + 1], waiting, width, now);
            if !self.folded(&Fold::Nested(top.key)) {
                for entry in nested {
                    self.session(out, entry, INDENT[depth + 2], 0, width, now);
                }
            }
        }
    }

    /// A worktree's age: its newest session's, else since git last changed it.
    fn worktree_age(&self, worktree: &Worktree, sessions: &[&Entry], now: Instant) -> String {
        match sessions.iter().map(|e| e.since).max() {
            Some(since) => age(since, now),
            None => self
                .tree
                .wall
                .zip(worktree.active)
                .and_then(|(wall, active)| wall.duration_since(active).ok())
                .map(age_of)
                .unwrap_or_default(),
        }
    }

    /// A session's row in the tree: its worktree is the row above, so it says no place.
    fn session(
        &self,
        out: &mut Rendered,
        entry: &Entry,
        indent: &'static str,
        waiting: usize,
        width: usize,
        now: Instant,
    ) {
        let row = Row {
            entry,
            indent,
            spin: self.spin,
            waiting,
            place: false,
            tag: None,
        };
        row.render(out, width, now);
    }

    /// `Other projects`, folded by default, by directory, each folded too.
    fn other(
        &self,
        out: &mut Rendered,
        other: &mut [(&Path, Vec<&Entry>)],
        width: usize,
        now: Instant,
    ) {
        if other.is_empty() {
            return;
        }
        let total: usize = other.iter().map(|(_, sessions)| sessions.len()).sum();
        let working = other
            .iter()
            .flat_map(|(_, sessions)| sessions)
            .filter(|e| e.state == State::Working)
            .count();
        let right = if working > 0 {
            format!("{} {working}  {total}", mark(State::Working))
        } else {
            total.to_string()
        };
        let folded = self.folded(&Fold::Group(Group::Other));
        let head = format!("{} ", if folded { '▸' } else { '▾' });
        let roll = Some((right, Highlight::SessionQuiet));
        heading(
            out,
            (&head, "Other projects"),
            roll,
            width,
            Line::Header(Group::Other),
        );
        if folded {
            return;
        }
        other.sort_by_cached_key(|(dir, _)| dir_name(dir));
        for (dir, sessions) in other.iter_mut() {
            let group = Group::OtherDir(dir.to_path_buf());
            let folded = self.folded(&Fold::Group(group.clone()));
            let head = format!("{}{} ", INDENT[1], if folded { '▸' } else { '▾' });
            let name = format!("{} ({})", dir_name(dir), sessions.len());
            heading(out, (&head, &name), None, width, Line::Header(group));
            if !folded {
                sessions.sort_by(|a, b| order(a).cmp(&order(b)));
                for entry in sessions.iter() {
                    self.session(out, entry, OTHER_SESSION, 0, width, now);
                }
            }
        }
    }

    /// Archived sessions, last and folded, each saying its place.
    fn archived(&self, out: &mut Rendered, archived: &[&Entry], width: usize, now: Instant) {
        if archived.is_empty() {
            return;
        }
        let folded = self.folded(&Fold::Group(Group::Archived));
        let head = format!("{} ", if folded { '▸' } else { '▾' });
        let name = format!("Archived {}", archived.len());
        heading(
            out,
            (&head, &name),
            None,
            width,
            Line::Header(Group::Archived),
        );
        if folded {
            return;
        }
        let mut archived = archived.to_vec();
        archived.sort_by(|a, b| order(a).cmp(&order(b)));
        for entry in archived {
            // At a session's indent, as every session row of the tree, so that only a heading
            // takes a worktree's (`sessions-next-heading`).
            let row = Row {
                entry,
                indent: SESSION,
                spin: None,
                waiting: 0,
                place: true,
                tag: None,
            };
            row.render(out, width, now);
        }
    }
}

/// What a worktree's row names: the main checkout's branch, marked `primary`, or a linked
/// worktree's directory, with its branch where that differs.
fn names(worktree: &Worktree) -> (String, Option<String>) {
    let dir = dir_name(&worktree.path);
    let branch = worktree.branch.as_deref().map(clean);
    if worktree.primary {
        return (branch.unwrap_or(dir), Some("primary".to_owned()));
    }
    let tag = branch.filter(|branch| *branch != dir);
    (dir, tag)
}

/// A heading row: `head` and `name`, and at the right edge `right` in its face.
fn heading(
    out: &mut Rendered,
    (head, name): (&str, &str),
    right: Option<(String, Highlight)>,
    width: usize,
    line: Line,
) {
    named_heading(out, (head, name, None), right, width, line);
}

/// A heading row as [`heading`] draws it, its name in `name_face` if given: the space in
/// front's.
fn named_heading(
    out: &mut Rendered,
    (head, name, name_face): (&str, &str, Option<Highlight>),
    right: Option<(String, Highlight)>,
    width: usize,
    line: Line,
) {
    let (right, right_face) = right.unwrap_or((String::new(), Highlight::SessionQuiet));
    let room = width.saturating_sub(cells(head) + cells(&right) + 1);
    let text = format!("{head}{}", cut_middle(super::unmarked(name), room));
    let name_end = text.len();
    let text = pad(&text, width.saturating_sub(cells(&right))) + &right;
    let mut spans = vec![(text.len() - right.len()..text.len(), right_face)];
    if let Some(face) = name_face {
        spans.insert(0, (head.len()..name_end, face));
    }
    out.push_drawn(&cut(&text, width), line, &spans);
}

/// Where `entry` goes: with the archived sessions when it or its space is; under the deepest
/// worktree of the tree that holds its directory, of its own space where two spaces show one;
/// else in `Other projects` by its repository, else by its directory. `worktrees` are the
/// tree's, by space, repository and worktree, the deepest first.
fn place<'e>(
    worktrees: &[(usize, usize, usize, &[u8])],
    tree: &Tree,
    entry: &'e Entry,
) -> Place<'e> {
    let closed = |id: &SpaceId| tree.closed.iter().any(|space| space.id == *id);
    // A closed space's sessions wait with the archived ones (ADR 0136, step 2).
    if entry.archived || entry.space_id.as_ref().is_some_and(closed) {
        return Place::Archived;
    }
    let Some(dir) = entry.dir.as_deref() else {
        return Place::Other(entry.repo.as_deref().unwrap_or(Path::new("")));
    };
    // A session of a space in the space's own directory: its orchestrator.
    let own_directory = tree.spaces.iter().position(|space| {
        let here = (space.directory.as_deref()).is_some_and(|own| dir.starts_with(own));
        entry.space_id.as_ref() == Some(&space.id) && here
    });
    if let Some(s) = own_directory {
        return Place::Orchestrator(s);
    }
    let bytes = dir.as_os_str().as_encoded_bytes();
    let mut holding = worktrees.iter().filter(|(.., path)| holds(path, bytes));
    let Some(&(s, p, w, deepest)) = holding.next() else {
        return Place::Other(entry.repo.as_deref().unwrap_or(dir));
    };
    // A main checkout two spaces show: the session's own space's.
    let own = |s: usize| entry.space_id.as_ref() == Some(&tree.spaces[s].id);
    if own(s) {
        return Place::Worktree(s, p, w);
    }
    let found = holding
        .take_while(|(.., path)| path.len() == deepest.len())
        .find(|(other, ..)| own(*other));
    match found {
        Some(&(s, p, w, _)) => Place::Worktree(s, p, w),
        None => Place::Worktree(s, p, w),
    }
}

/// Whether the path whose bytes are `path` holds the one whose bytes are `dir`: a byte prefix
/// that ends at a separator, as `Path::starts_with` has it, but cheaper.
fn holds(path: &[u8], dir: &[u8]) -> bool {
    dir.starts_with(path)
        && (dir.len() == path.len()
            || path.ends_with(b"/")
            || std::path::is_separator(char::from(dir[path.len()])))
}

impl Tree {
    /// The repository `root` as the first space that has it shows it.
    #[cfg(test)]
    pub fn project(&self, root: &Path) -> Option<&Project> {
        self.projects().find(|project| project.root == root)
    }

    /// The deepest worktree that holds `dir`, with its repository, as a session there is
    /// placed.
    pub fn holding(&self, dir: &Path) -> Option<(&Project, &Worktree)> {
        let bytes = dir.as_os_str().as_encoded_bytes();
        by_depth(&self.spaces)
            .into_iter()
            .find(|(.., path)| holds(path, bytes))
            .map(|(s, p, w, _)| {
                let project = &self.spaces[s].projects[p];
                (project, &project.worktrees[w])
            })
    }

    /// The worktree at `path` as space `id` shows it, with its repository.
    pub fn holding_in(&self, id: &SpaceId, path: &Path) -> Option<(&Project, &Worktree)> {
        let space = self.spaces.iter().find(|space| space.id == *id)?;
        space.projects.iter().find_map(|project| {
            let worktree = project.worktrees.iter().find(|w| w.path == path)?;
            Some((project, worktree))
        })
    }

    /// Every repository of every space, in their order.
    #[cfg(test)]
    pub fn projects(&self) -> impl Iterator<Item = &Project> {
        self.spaces.iter().flat_map(|space| space.projects.iter())
    }

    /// A tree of one space for each of `projects`, named and keyed after it, as a test of rows
    /// below the spaces needs.
    #[cfg(test)]
    pub fn of_projects(projects: Vec<Project>) -> Self {
        let spaces = (projects.into_iter().enumerate())
            .map(|(n, project)| SpaceRows {
                id: SpaceId::from_random([u8::try_from(n).unwrap_or(u8::MAX); 16]),
                name: project.name.clone(),
                projects: vec![project],
                directory: None,
                orchestrator: None,
            })
            .collect();
        Self {
            spaces,
            ..Self::default()
        }
    }
}

/// Every worktree of `spaces`, by space, repository and worktree with its path's bytes, the
/// deepest first, so that the first that holds a directory is the deepest.
fn by_depth(spaces: &[SpaceRows]) -> Vec<(usize, usize, usize, &[u8])> {
    let mut all: Vec<(usize, usize, usize, &[u8])> = Vec::new();
    for (s, space) in spaces.iter().enumerate() {
        for (p, project) in space.projects.iter().enumerate() {
            for (w, worktree) in project.worktrees.iter().enumerate() {
                all.push((s, p, w, worktree.path.as_os_str().as_encoded_bytes()));
            }
        }
    }
    all.sort_by_key(|(.., path)| std::cmp::Reverse(path.len()));
    all
}

/// The most urgent of `sessions`, oso's own before an external one in the same state.
fn worst<'e>(sessions: &[&'e Entry]) -> Option<&'e Entry> {
    sessions
        .iter()
        .copied()
        .min_by_key(|e| (rank(e.state), matches!(e.key, Key::External(_))))
}

/// `sessions` as families: each top-level one in order, oso's own first, with the sessions
/// nested under it among them. One whose chain of parents runs in a circle stays at the top.
fn families<'e>(sessions: &[&'e Entry]) -> Vec<(&'e Entry, Vec<&'e Entry>)> {
    let by_key: HashMap<Key, &Entry> = sessions.iter().map(|e| (e.key, *e)).collect();
    let top = |entry: &'e Entry| -> Key {
        let mut at = entry;
        let mut seen = HashSet::from([entry.key]);
        while let Some(parent) = at.parent.and_then(|key| by_key.get(&key)) {
            if !seen.insert(parent.key) {
                return entry.key;
            }
            at = parent;
        }
        at.key
    };
    let mut sorted = sessions.to_vec();
    sorted.sort_by(|a, b| {
        let external = |e: &Entry| matches!(e.key, Key::External(_));
        (external(a), order(a)).cmp(&(external(b), order(b)))
    });
    let mut families: Vec<(&Entry, Vec<&Entry>)> = Vec::new();
    for &entry in &sorted {
        if top(entry) == entry.key {
            families.push((entry, Vec::new()));
        }
    }
    for &entry in &sorted {
        let top = top(entry);
        if top != entry.key
            && let Some((_, nested)) = families.iter_mut().find(|(e, _)| e.key == top)
        {
            nested.push(entry);
        }
    }
    families
}

/// A directory's last name, as the tree shows it: cleaned as a recap is, as a path from git
/// or Claude Code's registry may hold any character.
fn dir_name(path: &Path) -> String {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    clean(&name)
}

#[cfg(test)]
mod tests;
