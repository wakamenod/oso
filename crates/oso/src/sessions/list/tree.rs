//! The tree of `*Sessions*` (ADR 0131, step 2): each project of this oso, its worktrees, the main
//! checkout first and then the newest made, and the sessions in each, the most urgent first. An
//! external session in a worktree of the tree is listed under it, quiet; every other one goes
//! in `Other projects`, by the repository or directory it runs in. A worktree with no session
//! and no change for a week goes in its project's quiet row. Rows never move when a state
//! changes: urgency shows in the marks, and a worktree's and a project's roll-up.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data in, text out, as the list.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use oso_core::syntax::Highlight;

use super::{
    Entry, Fold, Group, Key, Line, List, NESTED, PLACE_FROM, Rendered, Row, SPINNER, age, age_of,
    cells, cut, cut_middle, face, mark, order, pad, rank,
};
use crate::sessions::recap::clean;
use crate::sessions::state::State;

/// How long a worktree with no session stays unchanged before it goes in the quiet row.
pub const QUIET_AFTER: Duration = Duration::from_secs(7 * 86_400);

/// The mark of a worktree with no session: one of the row marks, so that `n` and `p` stop on it.
pub const NO_SESSION: char = '·';

/// How far a worktree's row is indented, and its sessions'.
const WORKTREE: &str = "  ";
const SESSION: &str = "    ";
const NESTED_SESSION: &str = "      ";

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

/// What the tree shows besides the sessions.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub projects: Vec<Project>,
    /// The worktree, or project root, whose space is in front.
    pub front: Option<PathBuf>,
    /// The wall-clock time the render's `now` stands for, for worktrees' activity.
    pub wall: Option<SystemTime>,
}

/// Where a session goes in the tree.
enum Place<'e> {
    /// Under worktree `.1` of project `.0`.
    Worktree(usize, usize),
    /// In `Other projects`, under this directory.
    Other(&'e Path),
    Archived,
}

/// A worktree's sessions, as placed.
type Placed<'e> = Vec<Vec<Vec<&'e Entry>>>;

impl List {
    /// The tree of `entries` for a window `width` cells wide, ages as of `now`.
    pub(super) fn render_tree(&self, entries: &[Entry], width: usize, now: Instant) -> Rendered {
        let mut out = Rendered::default();
        let projects = &self.tree.projects;
        let mut placed: Placed<'_> = projects
            .iter()
            .map(|project| vec![Vec::new(); project.worktrees.len()])
            .collect();
        let mut other: Vec<(&Path, Vec<&Entry>)> = Vec::new();
        // Each directory's place in `other`, by its bytes: cheaper than comparing paths.
        let mut other_at: HashMap<&[u8], usize> = HashMap::new();
        let mut archived = Vec::new();
        let worktrees = by_depth(projects);
        for entry in entries {
            match place(&worktrees, entry) {
                Place::Worktree(p, w) => placed[p][w].push(entry),
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
        for (project, worktrees) in projects.iter().zip(&placed) {
            self.project(&mut out, project, worktrees, width, now);
        }
        self.other(&mut out, &mut other, width, now);
        self.archived(&mut out, &archived, width, now);
        out
    }

    /// A project's row, then its worktrees unless it is folded.
    fn project(
        &self,
        out: &mut Rendered,
        project: &Project,
        placed: &[Vec<&Entry>],
        width: usize,
        now: Instant,
    ) {
        let all: Vec<&Entry> = placed.iter().flatten().copied().collect();
        let folded = self.folded_as(&Fold::Project(project.root.clone()), false);
        let roll = worst(&all).map(|top| {
            let n = all
                .iter()
                .filter(|e| rank(e.state) == rank(top.state))
                .count();
            (format!("{} {n}", mark(top.state)), face(top.state))
        });
        let head = format!("{} ", if folded { '▸' } else { '▾' });
        heading(
            out,
            (&head, &clean(&project.name)),
            roll,
            width,
            Line::Project(project.root.clone()),
        );
        if folded {
            return;
        }
        let mut shown: Vec<usize> = (0..project.worktrees.len()).collect();
        shown.sort_by_key(|&w| {
            let worktree = &project.worktrees[w];
            (!worktree.primary, std::cmp::Reverse(worktree.made))
        });
        let quiet_since = self
            .tree
            .wall
            .and_then(|wall| wall.checked_sub(QUIET_AFTER));
        let (quiet, shown): (Vec<usize>, Vec<usize>) = shown.into_iter().partition(|&w| {
            let worktree = &project.worktrees[w];
            placed[w].is_empty()
                && !worktree.primary
                && worktree
                    .active
                    .zip(quiet_since)
                    .is_some_and(|(active, since)| active < since)
        });
        for w in shown {
            self.worktree(out, &project.worktrees[w], &placed[w], width, now);
        }
        if quiet.is_empty() {
            return;
        }
        let group = Group::Quiet(project.root.clone());
        let folded = self.folded(&Fold::Group(group.clone()));
        let head = format!("{WORKTREE}{} ", if folded { '▸' } else { '▾' });
        let name = format!("{} quiet", quiet.len());
        heading(out, (&head, &name), None, width, Line::Header(group));
        if !folded {
            for w in quiet {
                self.worktree(out, &project.worktrees[w], &[], width, now);
            }
        }
    }

    /// A worktree's row, then its sessions unless it is folded: by default, while none of
    /// oso's runs in it.
    fn worktree(
        &self,
        out: &mut Rendered,
        worktree: &Worktree,
        sessions: &[&Entry],
        width: usize,
        now: Instant,
    ) {
        let line = Line::Worktree(worktree.path.clone());
        let hosted: Vec<&Entry> = sessions
            .iter()
            .copied()
            .filter(|e| matches!(e.key, Key::Hosted(_)))
            .collect();
        let running = hosted.iter().any(|e| e.state != State::Stopped);
        let folded = self.folded_as(&Fold::Worktree(worktree.path.clone()), !running);
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
        let age = self.worktree_age(worktree, sessions, now);
        let head = format!("{WORKTREE}{mark} ");
        let room = width.saturating_sub(cells(&head) + cells(&age) + cells(&count) + 1);
        let name = cut_middle(&name, room);
        let mut text = format!("{head}{name}");
        let mut spans = vec![(WORKTREE.len()..head.len() - 1, mark_face)];
        let front = self.tree.front.as_deref() == Some(worktree.path.as_path());
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
            self.session(out, top, SESSION, waiting, width, now);
            if !self.folded(&Fold::Nested(top.key)) {
                for entry in nested {
                    self.session(out, entry, NESTED_SESSION, 0, width, now);
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
            let head = format!("{WORKTREE}{} ", if folded { '▸' } else { '▾' });
            let name = format!("{} ({})", dir_name(dir), sessions.len());
            heading(out, (&head, &name), None, width, Line::Header(group));
            if !folded {
                sessions.sort_by(|a, b| order(a).cmp(&order(b)));
                for entry in sessions.iter() {
                    self.session(out, entry, SESSION, 0, width, now);
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
            let row = Row {
                entry,
                indent: NESTED,
                spin: None,
                waiting: 0,
                place: true,
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
    let (right, right_face) = right.unwrap_or((String::new(), Highlight::SessionQuiet));
    let room = width.saturating_sub(cells(head) + cells(&right) + 1);
    let text = format!("{head}{}", cut_middle(super::unmarked(name), room));
    let text = pad(&text, width.saturating_sub(cells(&right))) + &right;
    let spans = [(text.len() - right.len()..text.len(), right_face)];
    out.push_drawn(&cut(&text, width), line, &spans);
}

/// Where `entry` goes: under the deepest worktree of the tree that holds its directory, else in
/// `Other projects` by its repository, else by its directory. `worktrees` are the tree's, by
/// project and worktree, the deepest first.
fn place<'e>(worktrees: &[(usize, usize, &[u8])], entry: &'e Entry) -> Place<'e> {
    if entry.archived {
        return Place::Archived;
    }
    let Some(dir) = entry.dir.as_deref() else {
        return Place::Other(entry.repo.as_deref().unwrap_or(Path::new("")));
    };
    let bytes = dir.as_os_str().as_encoded_bytes();
    // A byte prefix that ends at a separator, as `Path::starts_with` has it, but cheaper.
    let holds = |path: &[u8]| {
        bytes.starts_with(path)
            && (bytes.len() == path.len()
                || path.ends_with(b"/")
                || std::path::is_separator(char::from(bytes[path.len()])))
    };
    match worktrees.iter().find(|(.., path)| holds(path)) {
        Some(&(p, w, _)) => Place::Worktree(p, w),
        None => Place::Other(entry.repo.as_deref().unwrap_or(dir)),
    }
}

/// Every worktree of `projects`, by project and worktree with its path's bytes, the deepest
/// first, so that the first that holds a directory is the deepest.
fn by_depth(projects: &[Project]) -> Vec<(usize, usize, &[u8])> {
    let mut all: Vec<(usize, usize, &[u8])> = projects
        .iter()
        .enumerate()
        .flat_map(|(p, project)| {
            project
                .worktrees
                .iter()
                .enumerate()
                .map(move |(w, worktree)| (p, w, worktree.path.as_os_str().as_encoded_bytes()))
        })
        .collect();
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
