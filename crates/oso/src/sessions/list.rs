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
    pub pinned: bool,
    pub archived: bool,
    /// An agent started it without asking (ADR 0135).
    pub unasked: bool,
    /// The session that started it.
    pub parent: Option<Key>,
    /// The directory it runs in, as the tree places it (ADR 0131): a hosted session's own, an
    /// external one's as the worker resolved it.
    pub dir: Option<PathBuf>,
    /// The repository root nearest above an external session's directory, for `Other
    /// projects`.
    pub repo: Option<PathBuf>,
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
    /// The spinner's frame, while working marks spin.
    spin: Option<usize>,
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

    /// What the pane's modeline says of it (ADR 0131): its grouping and its help key.
    pub fn modeline_note(&self) -> &'static str {
        match self.grouping {
            Grouping::Tree => "tree · ? keys",
            Grouping::State => "by state · ? keys",
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
        self.spin = frame.map(|frame| frame % SPINNER.len());
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
            Line::ClosedSpace(_) => return,
        };
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

    /// Whether `fold` is folded, when it is `default` until the person toggles it.
    fn folded_as(&self, fold: &Fold, default: bool) -> bool {
        default != self.toggled.contains(fold)
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
                    place: true,
                    tag: None,
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
                        place: true,
                        tag: None,
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

/// The mark of a session an agent started without asking (ADR 0135, as built).
const UNASKED: &str = "unasked";

/// One session's lines.
struct Row<'e> {
    entry: &'e Entry,
    indent: &'static str,
    /// The spinner's frame, while working marks spin.
    spin: Option<usize>,
    /// How many sessions nested under it need input.
    waiting: usize,
    /// Whether it shows its place: not under its worktree in the tree.
    place: bool,
    /// What it shows after its name in the tree instead: `orchestrator` (ADR 0136, step 3).
    tag: Option<&'static str>,
}

impl Row<'_> {
    fn render(&self, out: &mut Rendered, width: usize, now: Instant) {
        let entry = self.entry;
        let line = Line::Session(entry.key);
        let external = matches!(entry.key, Key::External(_));
        let spinning = !external && entry.state == State::Working && self.spin.is_some();
        let mark = match self.spin.filter(|_| spinning) {
            Some(frame) => SPINNER[frame],
            None => mark(entry.state),
        };
        let head = format!("{}{mark} ", self.indent);
        let age = shown_age(age(entry.since, now), width, cells(&head), &entry.name);
        // The name first, then its place where there is room, then the age at the right edge.
        let room = width.saturating_sub(cells(&head) + cells(&age) + 1);
        let name = cut_middle(&entry.name, room);
        let mut text = format!("{head}{name}");
        let left = room.saturating_sub(cells(&name) + 2);
        let mut place = None;
        // Under its worktree in the tree, only the mark of a start without asking.
        let shown: Option<std::borrow::Cow<'_, str>> = if let Some(tag) = self.tag {
            Some(tag.into())
        } else if self.place {
            Some(self.place().into())
        } else {
            entry.unasked.then_some(UNASKED.into())
        };
        if let Some(shown) = shown.filter(|_| left >= PLACE_FROM) {
            text.push_str("  ");
            let start = text.len();
            text.push_str(&cut_middle(unmarked(&shown), left));
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

    /// Where the session runs: its space and branch, whether it is pinned, and whether an agent
    /// started it without asking.
    fn place(&self) -> String {
        let entry = self.entry;
        let mut place = entry.space.clone();
        if let Some(branch) = &entry.branch {
            let _ = write!(place, " · {branch}");
        }
        if entry.pinned {
            place.push_str(" · pinned");
        }
        if entry.unasked {
            let _ = write!(place, " · {UNASKED}");
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

mod cells;
use cells::{cells, pad};
pub use cells::{cut, cut_middle, unmarked};

mod lines;
mod tree;

use lines::Fold;
pub use lines::{Group, Line};
pub use tree::{ClosedSpace, Project, SpaceInput, SpaceRows, Tree, Worktree, space_rows};

#[cfg(test)]
mod tests;
