//! `*Spaces*` (ADR 0121, Q6, step 3): the sessions grouped by state, in the taxonomy's order,
//! or by space; pinned ones first in a group; sessions nested under the session that started
//! them; archived ones in a last group, folded. Rendered to the window's width, so wrapping
//! never applies, with a map from each line to what it shows. A session takes one line at
//! any width (ADR 0131): its mark, its name cut in the middle, its place where there is room,
//! and its age at the right edge. Only a session that needs input or failed, or did not resume,
//! or has nested sessions that need input, takes a second line, its reason.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data in, text out, so
//! that the board ADR 0126 plans is a second renderer of the same model.
//!
//! Names, spaces, branches and recaps are untrusted text: each is cut to the room it has, and
//! counted in cells as oso draws it, so that no line runs past the width.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use oso_core::syntax::Highlight;

use super::state::State;
use super::worker::SessionKey;

/// The frames of a working session's spinning mark, in turn (ADR 0131, Q12): each one cell
/// wide, in the place of `▶`.
pub const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The marks a row of the list begins with, after its indent: `n` and `p` move between them.
pub use crate::commands::agent_sessions::ROW_MARKS as MARKS;

/// What a row of the list may have before its mark: the guides of a session's row (ADR 0140).
pub use crate::commands::agent_sessions::GUIDES;

/// The fewest cells a place is shown in after a name; with less room it is left out.
const PLACE_FROM: usize = 6;

/// Below this width, a row's age gives way to its name (ADR 0131, Q7).
const AGE_GIVES_WAY_BELOW: usize = 36;

/// The cells of a name that keep its row's age, in a pane under [`AGE_GIVES_WAY_BELOW`].
const NAME_KEPT: usize = 12;

/// What a row `width` wide shows of `age` after `head` cells: all of it, unless the pane is
/// under 36 columns and it would cut `name` below 12 cells, or below its length if shorter.
pub(super) fn shown_age(age: String, width: usize, head: usize, name: &str) -> String {
    if width >= AGE_GIVES_WAY_BELOW {
        return age;
    }
    let room = width.saturating_sub(head + cells(&age) + 1);
    if room >= cells(name).min(NAME_KEPT) {
        age
    } else {
        String::new()
    }
}

/// The first cell of every row, where the bar at point goes while the pane is selected
/// (ADR 0140).
pub const LEAD: &str = " ";

/// How far a nested session is indented under its parent, past [`LEAD`].
const NESTED: &str = "   ";

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
    /// Its space's name, as rows and completions show it.
    pub space: String,
    /// Its space, which places it in the tree (ADR 0136).
    pub space_id: Option<crate::sessions::spaces::SpaceId>,
    /// Its branch, in a trusted project only.
    pub branch: Option<String>,
    pub state: State,
    /// Since when it is in its state.
    pub since: Instant,
    pub recap: String,
    /// Its terminal's title, without a spinner: its label in the tree when its name is its
    /// worktree's (ADR 0140, Q2). Empty for another program's session.
    pub title: String,
    pub pinned: bool,
    pub archived: bool,
    /// An agent started it without asking (ADR 0135).
    pub unasked: bool,
    /// Why its last resume was refused (ADR 0165, "Worktrees' resumes"), under it while it is
    /// stopped.
    pub not_resumed: Option<String>,
    /// The session that started it.
    pub parent: Option<Key>,
    /// The directory it runs in, as the tree places it (ADR 0131): a hosted session's own, an
    /// external one's as the worker resolved it.
    pub dir: Option<PathBuf>,
    /// The repository root nearest above an external session's directory, for `Other
    /// projects`.
    pub repo: Option<PathBuf>,
}

impl Entry {
    /// What it says of itself: why it did not resume, while it is stopped, else its recap.
    pub fn said(&self) -> &str {
        match (&self.not_resumed, self.state) {
            (Some(why), State::Stopped) => why,
            _ => &self.recap,
        }
    }
}

/// How the list groups sessions; `s` switches (ADR 0131).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grouping {
    /// Projects, their worktrees, and the sessions in each.
    #[default]
    Tree,
    State,
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
    /// The lines that blink: a waiting session's, and a roll-up of one (ADR 0140, as amended).
    pub blinking: Vec<usize>,
    /// The lines drawn on a band (ADR 0140).
    pub bands: SpaceBands,
    /// How deep the tree shows open: 0 its spaces, 1 down to its worktrees, 2 a worktree's
    /// sessions too, from which `S-TAB` goes on (ADR 0140).
    pub open_to: u8,
}

/// The lines of the list drawn on a band (ADR 0140), by line from 0, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpaceBands {
    /// A section's row: a space's, `Other projects`', `Archived`'s.
    pub sections: Vec<usize>,
    /// The front worktree's rows.
    pub tint: Vec<usize>,
    /// On the lit beat of the blink, the cells of each blinking line drawn inverse: its line
    /// and its columns, by line in order (ADR 0140, as amended).
    pub lit: Vec<(usize, Range<usize>)>,
}

/// The pane's clock as a render reads it: the spinner's frame, whether the blink's beat is
/// lit, and the session in front of the person, whose row does not blink.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Beat {
    /// The spinner's frame, while working marks spin.
    pub(super) spin: Option<usize>,
    /// Whether the beat is lit, while waiting rows blink.
    pub(super) lit: Option<bool>,
    /// The session shown in the selected window of a focused oso window.
    pub(super) watched: Option<Key>,
}

impl Beat {
    /// The beat `entry`'s row blinks on, lit or not: a session of oso's that needs input, not
    /// archived and not the one in front of the person (ADR 0140, as amended). `None`: it
    /// stands still.
    pub(super) fn blinks(self, entry: &Entry) -> Option<bool> {
        let lit = self.lit?;
        let waits = matches!(entry.key, Key::Hosted(_))
            && matches!(entry.state, State::NeedsInput(_))
            && !entry.archived
            && self.watched != Some(entry.key);
        waits.then_some(lit)
    }

    /// The beat a roll-up of `sessions` blinks on: while one of them blinks, which needs input
    /// and so is its most urgent.
    pub(super) fn roll_blinks(self, sessions: &[&Entry]) -> Option<bool> {
        sessions.iter().find_map(|e| self.blinks(e))
    }
}

/// The list's own state: its grouping and what the person folded.
#[derive(Debug, Default)]
pub struct List {
    grouping: Grouping,
    /// Folds toggled from their default: the archived group starts folded, the rest open.
    toggled: HashSet<Fold>,
    /// Spaces the person folded or opened, kept so whatever their default comes to: it moves
    /// with the space in front and its sessions' states (ADR 0136, step 2, Q4).
    chosen: HashMap<crate::sessions::spaces::SpaceId, bool>,
    /// Whether each space was folded when the tree was last made, which `TAB` turns over.
    shown: std::cell::RefCell<HashMap<crate::sessions::spaces::SpaceId, bool>>,
    /// Folds `S-TAB` set, whatever their default: folded or open (ADR 0140). `TAB` turns one
    /// over.
    set: HashMap<Fold, bool>,
    /// The spinner's frame and the blink's beat.
    beat: Beat,
    /// What the tree shows besides sessions.
    tree: tree::Tree,
}

impl List {
    /// The list grouped by state, as `s` shows it.
    #[cfg(test)]
    pub fn by_state() -> Self {
        Self {
            grouping: Grouping::State,
            ..Self::default()
        }
    }

    /// What the pane's modeline says of it (ADR 0140): its name and grouping, then how to
    /// enter it, by the keys `enter` of `sessions` as bound (ADR 0143), or, while it is
    /// `selected`, how to leave it, fold and find its keys.
    pub fn modeline_note(&self, selected: bool, enter: &str) -> String {
        match (self.grouping, selected) {
            (Grouping::Tree, false) => format!("Spaces · {enter} to enter"),
            (Grouping::Tree, true) => "Spaces · q back · TAB fold · ? keys".to_owned(),
            (Grouping::State, false) => format!("Spaces by state · {enter} to enter"),
            (Grouping::State, true) => "Spaces by state · q back · ? keys".to_owned(),
        }
    }

    /// The projects and worktrees it shows, as last set.
    pub fn tree(&self) -> &tree::Tree {
        &self.tree
    }

    /// The projects and worktrees the tree shows, and the space in front (ADR 0131).
    pub fn set_tree(&mut self, tree: tree::Tree) {
        self.tree = tree;
    }

    /// Working marks spin, showing frame `frame` of [`SPINNER`], or stand still with `None`.
    pub fn set_spin(&mut self, frame: Option<usize>) {
        self.beat.spin = frame.map(|frame| frame % SPINNER.len());
    }

    /// Waiting rows blink, lit or not as `lit` says, but `watched`'s, or stand still with
    /// `None` (ADR 0140, as amended).
    pub fn set_blink(&mut self, lit: Option<bool>, watched: Option<Key>) {
        self.beat.lit = lit;
        self.beat.watched = watched;
    }

    /// `s`: the tree, or by state.
    pub fn regroup(&mut self) {
        self.grouping = match self.grouping {
            Grouping::Tree => Grouping::State,
            Grouping::State => Grouping::Tree,
        };
    }

    /// `TAB` on `line`: folds or opens its group, space, repository or worktree, or the
    /// session's nested sessions. A closed space's row folds nothing.
    pub fn fold(&mut self, line: &Line) {
        let fold = match line {
            Line::Header(group) => Fold::Group(group.clone()),
            Line::Session(key) => Fold::Nested(*key),
            Line::Space(id) => {
                let folded = self.shown.borrow().get(id).copied().unwrap_or(false);
                self.chosen.insert(id.clone(), !folded);
                return;
            }
            Line::Repository(id, root) => Fold::Repository(id.clone(), root.clone()),
            Line::Worktree(id, path) => Fold::Worktree(id.clone(), path.clone()),
            Line::ClosedSpace(_) | Line::Title => return,
        };
        if let Some(folded) = self.set.get_mut(&fold) {
            *folded = !*folded;
            return;
        }
        if !self.toggled.remove(&fold) {
            self.toggled.insert(fold);
        }
    }

    fn folded(&self, fold: &Fold) -> bool {
        let default = matches!(
            fold,
            Fold::Group(
                Group::Archived
                    | Group::ArchivedSpaces
                    | Group::Other
                    | Group::OtherDir(_)
                    | Group::Quiet(..)
            )
        );
        self.folded_as(fold, default)
    }

    /// Whether space `id` is folded: as the person chose, else `default`. Noted for `TAB`.
    fn space_folded(&self, id: &crate::sessions::spaces::SpaceId, default: bool) -> bool {
        let folded = self.chosen.get(id).copied().unwrap_or(default);
        self.shown.borrow_mut().insert(id.clone(), folded);
        folded
    }

    /// Whether `fold` is folded, when it is `default` until the person toggles it or `S-TAB`
    /// sets it.
    fn folded_as(&self, fold: &Fold, default: bool) -> bool {
        match self.set.get(fold) {
            Some(folded) => *folded,
            None => default != self.toggled.contains(fold),
        }
    }

    /// `S-TAB` (ADR 0140): the tree folded to its spaces, then open down to its worktrees,
    /// then everything, in turn, from `open_to`, how deep it showed open when last rendered
    /// ([`Rendered::open_to`]). False, and nothing folds, while the list is by state.
    pub fn cycle_folds(&mut self, open_to: u8) -> bool {
        if self.grouping != Grouping::Tree {
            return false;
        }
        let next = (open_to + 1) % 3;
        for space in &self.tree.spaces {
            self.chosen.insert(space.id.clone(), next == 0);
            if next == 0 {
                continue;
            }
            for project in &space.projects {
                let repository = Fold::Repository(space.id.clone(), project.root.clone());
                self.set.insert(repository, false);
                for worktree in &project.worktrees {
                    let fold = Fold::Worktree(space.id.clone(), worktree.path.clone());
                    self.set.insert(fold, next == 1);
                }
            }
        }
        true
    }

    /// The list of `entries` for a window `width` cells wide, ages as of `now`: the tree, or
    /// grouped by state.
    pub fn render(&self, entries: &[Entry], width: usize, now: Instant) -> Rendered {
        if self.grouping == Grouping::Tree {
            return self.render_tree(entries, width, now);
        }
        let mut out = Rendered::default();
        let groups = self.groups(entries);
        for (group, members) in &groups {
            let folded = self.folded(&Fold::Group(group.clone()));
            let header = format!(
                "{LEAD}{} {} {}",
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
                    place: true,
                    ..Row::new(parent, LEAD, self.beat, waiting)
                };
                row.render(&mut out, width, now);
                if self.folded(&Fold::Nested(parent.key)) {
                    continue;
                }
                for entry in nested {
                    let row = Row {
                        place: true,
                        ..Row::new(entry, NESTED, self.beat, 0)
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
        let group_of = |entry: &Entry| match entry.archived {
            true => Group::Archived,
            false => Group::State(rank(entry.state)),
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
        groups.sort_by_key(|(group, _)| group_order(group));
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

/// The order of groups by state: the taxonomy's, archived last.
fn group_order(group: &Group) -> u8 {
    match group {
        Group::State(rank) => *rank,
        _ => u8::MAX,
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
        Group::Other => "Other projects",
        Group::Quiet(..) => "quiet",
        Group::OtherDir(_) => "",
        Group::Archived => "Archived",
        Group::ArchivedSpaces => "Archived spaces",
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

impl Rendered {
    fn push(&mut self, text: &str, line: Line) {
        self.push_drawn(text, line, &[]);
    }

    /// Pushes `text` as a line, with `spans` of it, by its bytes, drawn as each says. A span
    /// is cut to the whole characters of the text kept.
    fn push_drawn(&mut self, text: &str, line: Line, spans: &[(Range<usize>, Highlight)]) {
        let text = text.trim_end();
        let base = self.text.len();
        // A span cut with the text keeps whole characters only: a cut may put its `…` where
        // a span began, and a part of it would be no character's face.
        let ceil =
            |at: usize| (at.min(text.len())..=text.len()).find(|&at| text.is_char_boundary(at));
        let floor = |at: usize| {
            (0..=at.min(text.len()))
                .rev()
                .find(|&at| text.is_char_boundary(at))
        };
        for (range, highlight) in spans {
            let (Some(start), Some(end)) = (ceil(range.start), floor(range.end)) else {
                continue;
            };
            let range = start..end;
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

mod cells;
use cells::{cells, pad};
pub use cells::{cut, cut_middle, unmarked};

mod lines;
mod row;
mod tree;

use row::Row;

use lines::Fold;
pub use lines::{Group, Line};
pub use tree::{ClosedSpace, Project, SpaceInput, SpaceRows, Tree, Worktree, space_rows};

#[cfg(test)]
mod tests;
