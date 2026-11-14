//! `*Sessions*` (ADR 0121, Q6, step 3): the sessions grouped by state, in the taxonomy's order,
//! or by space; pinned ones first in a group; sessions nested under the session that started
//! them; archived ones in a last group, folded. Rendered to the window's width, so wrapping
//! never applies, with a map from each line to what it shows. A session takes one line at
//! any width (ADR 0131): its mark, its name cut in the middle, its place where there is room,
//! and its age at the right edge. Only a session that needs input or failed, or has nested
//! sessions that need input, takes a second line, its reason.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data in, text out, so
//! that the board ADR 0126 plans is a second renderer of the same model.
//!
//! Names, spaces, branches and recaps are untrusted text: each is cut to the room it has, and
//! counted in cells as oso draws it, so that no line runs past the width.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::ops::Range;
use std::time::{Duration, Instant};

use oso_core::display_map::cell::classify;
use oso_core::syntax::Highlight;

use super::state::State;
use super::worker::SessionKey;

/// The frames of a working session's spinning mark, in turn (ADR 0131, Q12): each one cell
/// wide, in the place of `▶`.
pub const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The marks a row of the list begins with, after its indent: `n` and `p` move between them.
pub use crate::commands::agent_sessions::ROW_MARKS as MARKS;

/// The fewest cells a place is shown in after a name; with less room it is left out.
const PLACE_FROM: usize = 6;

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
    /// What each range of the text is drawn as, by its bytes, in order (ADR 0131).
    pub highlights: Vec<(Range<usize>, Highlight)>,
    /// The lines whose mark spins.
    pub spinning: Vec<usize>,
}

/// The list's own state: its grouping and what the person folded.
#[derive(Debug, Default)]
pub struct List {
    grouping: Grouping,
    /// Folds toggled from their default: the archived group starts folded, the rest open.
    toggled: HashSet<Fold>,
    /// The spinner's frame, while working marks spin.
    spin: Option<usize>,
}

impl List {
    /// Working marks spin, showing frame `frame` of [`SPINNER`], or stand still with `None`.
    pub fn set_spin(&mut self, frame: Option<usize>) {
        self.spin = frame.map(|frame| frame % SPINNER.len());
    }

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
                    spin: self.spin,
                    waiting,
                };
                row.render(&mut out, width, now);
                if self.folded(&Fold::Nested(parent.key)) {
                    continue;
                }
                for entry in nested {
                    let row = Row {
                        entry,
                        indent: NESTED,
                        spin: self.spin,
                        waiting: 0,
                    };
                    row.render(&mut out, width, now);
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
            let mut seen = HashSet::from([entry.key]);
            let group = group_of(entry);
            while let Some(parent) = at.parent.and_then(|key| by_key.get(&key)) {
                if !seen.insert(parent.key) {
                    return entry;
                }
                if group_of(parent) != group {
                    break;
                }
                at = parent;
            }
            at
        };
        // Each entry's top, worked out once.
        let tops: HashMap<Key, Key> = entries
            .iter()
            .map(|entry| (entry.key, top(entry).key))
            .collect();
        let mut groups: Vec<(Group, Vec<&Entry>)> = Vec::new();
        let mut group_at: HashMap<Group, usize> = HashMap::new();
        for entry in entries {
            let group = group_of(entry);
            match group_at.get(&group) {
                Some(&at) => groups[at].1.push(entry),
                None => {
                    group_at.insert(group.clone(), groups.len());
                    groups.push((group, vec![entry]));
                }
            }
        }
        groups.sort_by(|(a, _), (b, _)| group_order(a).cmp(&group_order(b)));
        groups
            .into_iter()
            .map(|(group, mut members)| {
                members.sort_by(|a, b| order(a).cmp(&order(b)));
                let mut families: Vec<Family<'_>> = Vec::new();
                let mut family_of: HashMap<Key, usize> = HashMap::new();
                for &entry in &members {
                    if tops.get(&entry.key) == Some(&entry.key) {
                        family_of.insert(entry.key, families.len());
                        families.push((entry, Vec::new()));
                    }
                }
                for &entry in &members {
                    let Some(&top) = tops.get(&entry.key) else {
                        continue;
                    };
                    if top != entry.key
                        && let Some(&at) = family_of.get(&top)
                    {
                        families[at].1.push(entry);
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

/// An age as the list shows it: `now` under a minute, then `5m`, `3h`, `2d` (ADR 0131), so
/// that a row's text changes at most once a minute.
pub fn age(since: Instant, now: Instant) -> String {
    let ago = now.saturating_duration_since(since);
    if ago < Duration::from_secs(60) {
        return "now".to_owned();
    }
    age_of(ago)
}

/// A time gone by as the list shows it, as [`age`].
pub fn age_of(ago: Duration) -> String {
    let seconds = ago.as_secs();
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
    /// The spinner's frame, while working marks spin.
    spin: Option<usize>,
    /// How many sessions nested under it need input.
    waiting: usize,
}

impl Row<'_> {
    fn render(&self, out: &mut Rendered, width: usize, now: Instant) {
        let entry = self.entry;
        let line = Line::Session(entry.key);
        let external = matches!(entry.key, Key::External(_));
        let age = age(entry.since, now);
        let spinning = !external && entry.state == State::Working && self.spin.is_some();
        let mark = match self.spin.filter(|_| spinning) {
            Some(frame) => SPINNER[frame],
            None => mark(entry.state),
        };
        let head = format!("{}{mark} ", self.indent);
        // The name first, then its place where there is room, then the age at the right edge.
        let room = width.saturating_sub(cells(&head) + cells(&age) + 1);
        let name = cut_middle(&entry.name, room);
        let mut text = format!("{head}{name}");
        let left = room.saturating_sub(cells(&name) + 2);
        let mut place = None;
        if left >= PLACE_FROM {
            text.push_str("  ");
            let start = text.len();
            text.push_str(&cut_middle(unmarked(&self.place()), left));
            place = Some(start..text.len());
        }
        let text = pad(&text, width.saturating_sub(cells(&age))) + &age;
        let age_at = text.len() - age.len()..text.len();
        // A session run outside oso is quiet throughout: oso can mention it, never show it.
        let spans = if external {
            vec![(0..text.len(), Highlight::SessionQuiet)]
        } else {
            let at = self.indent.len();
            let mut spans = vec![(at..at + mark.len_utf8(), face(entry.state))];
            spans.extend(place.map(|place| (place, Highlight::SessionQuiet)));
            spans.push((age_at, Highlight::SessionQuiet));
            spans
        };
        if spinning {
            out.spinning.push(out.lines.len());
        }
        out.push_drawn(&cut(&text, width), line.clone(), &spans);
        // A reason only where the person has something to do.
        let own = matches!(entry.state, State::NeedsInput(_) | State::Failed)
            .then_some(entry.recap.as_str())
            .filter(|recap| !recap.is_empty());
        let reason = match (self.waiting, own) {
            (0, None) => return,
            (0, Some(own)) => own.to_owned(),
            (1, own) => joined("1 nested needs input", own),
            (n, own) => joined(&format!("{n} nested need input"), own),
        };
        let under = format!("{}  ", self.indent);
        let text = cut(&format!("{under}{}", unmarked(&reason)), width);
        let quiet = [(under.len()..text.len(), Highlight::SessionQuiet)];
        out.push_drawn(&text, line, &quiet);
    }

    /// Where the session runs: its space and branch, and whether it is pinned.
    fn place(&self) -> String {
        let entry = self.entry;
        let mut place = entry.space.clone();
        if let Some(branch) = &entry.branch {
            let _ = write!(place, " · {branch}");
        }
        if entry.pinned {
            place.push_str(" · pinned");
        }
        place
    }
}

/// What a state's mark is drawn as (ADR 0131, Q5): the person's attention, work under way,
/// work to look at, or nothing to do.
pub fn face(state: State) -> Highlight {
    match state {
        State::NeedsInput(_) | State::Failed => Highlight::SessionAlert,
        State::Working => Highlight::SessionWorking,
        State::ReadyForReview | State::Done => Highlight::SessionDone,
        State::Idle | State::Stopped => Highlight::SessionQuiet,
    }
}

/// `first`, then ` · ` and `then` if there is one.
fn joined(first: &str, then: Option<&str>) -> String {
    then.map_or_else(|| first.to_owned(), |then| format!("{first} · {then}"))
}

impl Rendered {
    fn push(&mut self, text: &str, line: Line) {
        self.push_drawn(text, line, &[]);
    }

    /// Pushes `text` as a line, with `spans` of it, by its bytes, drawn as each says. A span
    /// is cut to the text kept.
    fn push_drawn(&mut self, text: &str, line: Line, spans: &[(Range<usize>, Highlight)]) {
        let text = text.trim_end();
        let base = self.text.len();
        for (range, highlight) in spans {
            let range = range.start.min(text.len())..range.end.min(text.len());
            if !range.is_empty() {
                self.highlights
                    .push((base + range.start..base + range.end, *highlight));
            }
        }
        self.text.push_str(text);
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

/// `text` without what it begins with that could pass for a row's start: blanks, marks and
/// characters that take no cell.
pub fn unmarked(text: &str) -> &str {
    text.trim_start_matches(|c: char| c.is_whitespace() || MARKS.contains(c) || char_cells(c) == 0)
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

/// `text` cut to `width` cells in the middle, keeping its last third (ADR 0131), so that names
/// that share a long start still differ: `chore-survey-r…lifecycle-hook`. A line break ends it.
pub fn cut_middle(text: &str, width: usize) -> String {
    let text = text.lines().next().unwrap_or_default();
    if cells(text) <= width || width < 6 {
        return cut(text, width);
    }
    let tail_room = (width - 1) / 3;
    let mut tail = Vec::new();
    let mut used = 0;
    for c in text.chars().rev() {
        let n = char_cells(c);
        if used + n > tail_room {
            break;
        }
        tail.push(c);
        used += n;
    }
    let head_room = width - 1 - used;
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let n = char_cells(c);
        if used + n > head_room {
            break;
        }
        out.push(c);
        used += n;
    }
    out.push('…');
    out.extend(tail.into_iter().rev());
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
