//! `*Sessions*` (ADR 0121, Q6, step 3): the sessions grouped by state, in the taxonomy's order,
//! or by space; pinned ones first in a group; sessions nested under the session that started
//! them; archived ones in a last group, folded. Rendered to the window's width, so wrapping
//! never applies, with a map from each line to what it shows.
//!
//! Must not depend on: `app`, `commands`, `dispatch` or `config`; plain data in, text out, so
//! that the board ADR 0126 plans is a second renderer of the same model.
//!
//! Names, spaces, branches and recaps are untrusted text: each is cut to the room it has, and
//! counted in cells as oso draws it, so that no line runs past the width.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::time::Instant;

use oso_core::display_map::cell::classify;

use super::state::State;
use super::worker::SessionKey;

/// The narrowest window that shows a session on one line; narrower ones take three.
pub const WIDE_FROM: usize = 60;

/// The columns of a wide line's name and of its space and branch.
const NAME_CELLS: usize = 20;
const PLACE_CELLS: usize = 28;

/// How far a nested session is indented under its parent.
const NESTED: &str = "  ";

/// A row of the list: a session oso hosts, or one it read from Claude Code's registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Hosted(SessionKey),
    /// Another program's session, by its process id: listed, never shown.
    External(u32),
}

/// What the list shows of one session. The app fills it from the hosted table and the worker.
#[derive(Clone, Debug)]
pub struct Entry {
    pub key: Key,
    pub name: String,
    /// Its space's name: `repo`, or `repo/slug` for a worktree.
    pub space: String,
    /// Its branch, in a trusted project only.
    pub branch: Option<String>,
    pub state: State,
    /// Since when it is in its state.
    pub since: Instant,
    pub recap: String,
    pub pinned: bool,
    pub archived: bool,
    /// The session that started it.
    pub parent: Option<Key>,
}

/// How the list groups sessions; `s` switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grouping {
    #[default]
    State,
    Space,
}

/// A group of the list.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// Sessions in one state, by its rank in the taxonomy.
    State(u8),
    Space(String),
    Archived,
}

/// What a line of the list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Header(Group),
    Session(Key),
}

/// What `TAB` folds: a group, or a session's nested sessions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Fold {
    Group(Group),
    Nested(Key),
}

/// The list, rendered: its text and, for each line, what it shows.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub lines: Vec<Line>,
}

/// The list's own state: its grouping and what the person folded.
#[derive(Debug, Default)]
pub struct List {
    grouping: Grouping,
    /// Folds toggled from their default: the archived group starts folded, the rest open.
    toggled: HashSet<Fold>,
}

impl List {
    /// `s`: by state, or by space.
    pub fn regroup(&mut self) {
        self.grouping = match self.grouping {
            Grouping::State => Grouping::Space,
            Grouping::Space => Grouping::State,
        };
    }

    /// `TAB` on `line`: folds or opens its group, or the session's nested sessions.
    pub fn fold(&mut self, line: &Line) {
        let fold = match line {
            Line::Header(group) => Fold::Group(group.clone()),
            Line::Session(key) => Fold::Nested(*key),
        };
        if !self.toggled.remove(&fold) {
            self.toggled.insert(fold);
        }
    }

    fn folded(&self, fold: &Fold) -> bool {
        let default = *fold == Fold::Group(Group::Archived);
        default != self.toggled.contains(fold)
    }

    /// The list of `entries` for a window `width` cells wide, ages as of `now`.
    pub fn render(&self, entries: &[Entry], width: usize, now: Instant) -> Rendered {
        let mut out = Rendered::default();
        let groups = self.groups(entries);
        let wide = width >= WIDE_FROM;
        for (group, members) in &groups {
            let folded = self.folded(&Fold::Group(group.clone()));
            let header = format!(
                "{} {} {}",
                if folded { '▸' } else { '▾' },
                group_name(group),
                members
                    .iter()
                    .map(|(_, nested)| 1 + nested.len())
                    .sum::<usize>()
            );
            out.push(&cut(&header, width), Line::Header(group.clone()));
            if folded {
                continue;
            }
            for (parent, nested) in members {
                let waiting = nested
                    .iter()
                    .filter(|entry| matches!(entry.state, State::NeedsInput(_)))
                    .count();
                let row = Row {
                    entry: parent,
                    indent: "",
                    waiting,
                    nested: nested.len(),
                };
                row.render(&mut out, width, wide, now);
                if self.folded(&Fold::Nested(parent.key)) {
                    continue;
                }
                for entry in nested {
                    let row = Row {
                        entry,
                        indent: NESTED,
                        waiting: 0,
                        nested: 0,
                    };
                    row.render(&mut out, width, wide, now);
                }
            }
        }
        out
    }

    /// The groups in their order, each with its top-level sessions in order, and under each
    /// the sessions nested under it, in order.
    fn groups<'e>(&self, entries: &'e [Entry]) -> Vec<(Group, Vec<Family<'e>>)> {
        let group_of = |entry: &Entry| match (entry.archived, self.grouping) {
            (true, _) => Group::Archived,
            (false, Grouping::State) => Group::State(rank(entry.state)),
            (false, Grouping::Space) => Group::Space(entry.space.clone()),
        };
        let by_key: HashMap<Key, &Entry> = entries.iter().map(|entry| (entry.key, entry)).collect();
        // A session nests under its oldest ancestor in the list and in its group, so that a
        // chain of starts reads as one family. One whose chain of parents runs in a circle,
        // which only made-up parents do, stays at the top level.
        let top = |entry: &'e Entry| -> &'e Entry {
            let mut at = entry;
            let mut seen = vec![entry.key];
            while let Some(parent) = at.parent.and_then(|key| by_key.get(&key)) {
                if seen.contains(&parent.key) {
                    return entry;
                }
                if group_of(parent) != group_of(entry) {
                    break;
                }
                seen.push(parent.key);
                at = parent;
            }
            at
        };
        let mut groups: Vec<(Group, Vec<&Entry>)> = Vec::new();
        for entry in entries {
            let group = group_of(entry);
            match groups.iter_mut().find(|(g, _)| *g == group) {
                Some((_, members)) => members.push(entry),
                None => groups.push((group, vec![entry])),
            }
        }
        groups.sort_by(|(a, _), (b, _)| group_order(a).cmp(&group_order(b)));
        groups
            .into_iter()
            .map(|(group, mut members)| {
                members.sort_by(|a, b| order(a).cmp(&order(b)));
                let mut families: Vec<Family<'_>> = Vec::new();
                for &entry in &members {
                    if std::ptr::eq(top(entry), entry) {
                        families.push((entry, Vec::new()));
                    }
                }
                for &entry in &members {
                    let top = top(entry);
                    if !std::ptr::eq(top, entry)
                        && let Some((_, nested)) =
                            families.iter_mut().find(|(t, _)| std::ptr::eq(*t, top))
                    {
                        nested.push(entry);
                    }
                }
                (group, families)
            })
            .collect()
    }
}

/// A top-level session, and the sessions nested under it.
type Family<'e> = (&'e Entry, Vec<&'e Entry>);

/// A session's order in its group: pinned first, then the most urgent, then the one in its
/// state longest, then by name.
fn order(entry: &Entry) -> (bool, u8, Instant, &str) {
    (!entry.pinned, rank(entry.state), entry.since, &entry.name)
}

/// The order of groups: states in the taxonomy's order, spaces by name, archived last.
fn group_order(group: &Group) -> (u8, &str) {
    match group {
        Group::State(rank) => (*rank, ""),
        Group::Space(name) => (0, name),
        Group::Archived => (u8::MAX, ""),
    }
}

/// A state's rank in the taxonomy, most urgent first (ADR 0121).
pub fn rank(state: State) -> u8 {
    match state {
        State::NeedsInput(_) => 0,
        State::Failed => 1,
        State::Working => 2,
        State::ReadyForReview => 3,
        State::Done => 4,
        State::Idle => 5,
        State::Stopped => 6,
    }
}

/// A state's mark (ADR 0121, Q5): glyphs only, no faces.
pub fn mark(state: State) -> char {
    match state {
        State::NeedsInput(_) => '⚠',
        State::Failed => '✗',
        State::Working => '▶',
        State::ReadyForReview => '◆',
        State::Done => '●',
        State::Idle => '○',
        State::Stopped => '■',
    }
}

fn group_name(group: &Group) -> &str {
    match group {
        Group::State(0) => "Needs input",
        Group::State(1) => "Failed",
        Group::State(2) => "Working",
        Group::State(3) => "Ready for review",
        Group::State(4) => "Done",
        Group::State(5) => "Idle",
        Group::State(_) => "Stopped",
        Group::Space(name) => name,
        Group::Archived => "Archived",
    }
}

/// An age as the list shows it: `12s`, `5m`, `3h`, `2d`.
pub fn age(since: Instant, now: Instant) -> String {
    let seconds = now.saturating_duration_since(since).as_secs();
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// One session's lines.
struct Row<'e> {
    entry: &'e Entry,
    indent: &'static str,
    /// How many sessions nested under it need input, and how many there are.
    waiting: usize,
    nested: usize,
}

impl Row<'_> {
    fn render(&self, out: &mut Rendered, width: usize, wide: bool, now: Instant) {
        let entry = self.entry;
        let line = Line::Session(entry.key);
        let age = age(entry.since, now);
        let head = format!("{}{} ", self.indent, mark(entry.state));
        let mut place = entry.space.clone();
        if let Some(branch) = &entry.branch {
            let _ = write!(place, " · {branch}");
        }
        for (on, word) in [
            (entry.pinned, "pinned"),
            (matches!(entry.key, Key::External(_)), "external"),
        ] {
            if on {
                let _ = write!(place, " · {word}");
            }
        }
        let recap = match (self.waiting, self.nested) {
            (0, 0) => entry.recap.clone(),
            (0, n) => format!("{n} nested · {}", entry.recap),
            (1, _) => format!("1 nested needs input · {}", entry.recap),
            (n, _) => format!("{n} nested need input · {}", entry.recap),
        };
        if wide {
            let mut text = head;
            text.push_str(&pad(&cut(&entry.name, NAME_CELLS), NAME_CELLS + 1));
            text.push_str(&pad(&cut(&place, PLACE_CELLS), PLACE_CELLS + 1));
            text.push_str(&pad(&age, 4));
            text.push_str("  ");
            text.push_str(&recap);
            out.push(&cut(&text, width), line);
            return;
        }
        // The name, then the age at the right edge.
        let room = width.saturating_sub(cells(&head) + cells(&age) + 1);
        let name = cut(&entry.name, room);
        let first = format!("{head}{}{age}", pad(&name, room + 1));
        out.push(&cut(&first, width), line.clone());
        let under = format!("{}  ", self.indent);
        out.push(&cut(&format!("{under}{place}"), width), line.clone());
        out.push(&cut(&format!("{under}{recap}"), width), line);
    }
}

impl Rendered {
    fn push(&mut self, text: &str, line: Line) {
        self.text.push_str(text.trim_end());
        self.text.push('\n');
        self.lines.push(line);
    }
}

/// How many cells `text` takes, each character as oso draws it alone: a cluster of several
/// characters takes at most this, so that a line cut by it never runs past its width.
fn cells(text: &str) -> usize {
    text.chars().map(char_cells).sum()
}

fn char_cells(c: char) -> usize {
    let mut buffer = [0; 4];
    classify(c.encode_utf8(&mut buffer)).1
}

/// `text` cut to `width` cells, ending in `…` if cut; a line break ends it.
pub fn cut(text: &str, width: usize) -> String {
    let text = text.lines().next().unwrap_or_default();
    if cells(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let n = char_cells(c);
        if used + n + 1 > width {
            break;
        }
        out.push(c);
        used += n;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// `text` with spaces after it to `width` cells, if it is shorter.
fn pad(text: &str, width: usize) -> String {
    let mut out = text.to_owned();
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(cells(text))));
    out
}

#[cfg(test)]
mod tests;
