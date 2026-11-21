//! A session's row of `*Spaces*` (ADR 0131, ADR 0140): its indent and guides, its mark, its
//! label cut in the middle, its place where there is room, and its age at the right edge; and
//! a second line, its reason, for a session that needs input or failed, or has nested sessions
//! that need input.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data in, text out, as the list.

use std::fmt::Write as _;
use std::time::Instant;

use oso_core::syntax::Highlight;

use super::{
    Beat, Entry, Key, Line, PLACE_FROM, Rendered, SPINNER, age, cells, cut, cut_middle, face, mark,
    pad, shown_age, unmarked,
};
use crate::sessions::state::State;

/// The mark of a session an agent started without asking (ADR 0135, as built).
const UNASKED: &str = "unasked";

/// One session's lines.
pub(super) struct Row<'e> {
    pub(super) entry: &'e Entry,
    /// What comes before its mark: its indent, and in the tree the guides that hang it from its
    /// worktree's row (ADR 0140).
    pub(super) indent: &'e str,
    /// What comes before its reason, on the line under it: past its indent and guides, under
    /// its label. `None`: its indent and two cells.
    pub(super) under: Option<&'e str>,
    /// The spinner's frame and the blink's beat.
    pub(super) beat: Beat,
    /// How many sessions nested under it need input.
    pub(super) waiting: usize,
    /// Whether it shows its place: not under its worktree in the tree.
    pub(super) place: bool,
    /// What it shows after its name in the tree instead: `orchestrator` (ADR 0136, step 3).
    pub(super) tag: Option<&'static str>,
    /// What it is called on its row, if not its name: its title (ADR 0140, Q2).
    pub(super) label: Option<&'e str>,
}

impl<'e> Row<'e> {
    /// The row of `entry` after `indent`, its marks spinning and blinking on `beat`, with
    /// `waiting` of its nested sessions needing input: no place, no tag, its name.
    pub(super) fn new(entry: &'e Entry, indent: &'e str, beat: Beat, waiting: usize) -> Self {
        Self {
            entry,
            indent,
            under: None,
            beat,
            waiting,
            place: false,
            tag: None,
            label: None,
        }
    }

    pub(super) fn render(&self, out: &mut Rendered, width: usize, now: Instant) {
        let entry = self.entry;
        let line = Line::Session(entry.key);
        let external = matches!(entry.key, Key::External(_));
        let spin = self.beat.spin;
        let spinning = !external && entry.state == State::Working && spin.is_some();
        let mark = match spin.filter(|_| spinning) {
            Some(frame) => SPINNER[frame],
            None => mark(entry.state),
        };
        let label = self.label.unwrap_or(&entry.name);
        let head = format!("{}{mark} ", self.indent);
        let age = shown_age(age(entry.since, now), width, cells(&head), label);
        // The label first, then its place where there is room, then the age at the right edge.
        let room = width.saturating_sub(cells(&head) + cells(&age) + 1);
        let name = cut_middle(unmarked(label), room);
        let mut text = format!("{head}{name}");
        // Its mark and name, which a blink's lit beat draws inverse (ADR 0140, as amended):
        // counted in cells only then, as a row is not counted twice for nothing.
        let blink = self.beat.blinks(entry);
        let named = self.indent.len()..text.len();
        let named_cells = (blink == Some(true)).then(|| cells(self.indent)..cells(&text));
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
        let at = self.indent.len();
        // A session run outside oso is quiet throughout: oso can mention it, never show it.
        let spans = if external {
            vec![(0..text.len(), Highlight::SessionQuiet)]
        } else {
            // The guides quiet, where there are any: a span over blanks would cost a tick for
            // nothing.
            let guided = self.indent.trim_start().len();
            let mut spans = Vec::with_capacity(4);
            if guided > 0 {
                spans.push((at - guided..at, Highlight::SessionQuiet));
            }
            if blink == Some(true) {
                spans.push((named, Highlight::SessionAlertLit));
            } else {
                spans.push((at..at + mark.len_utf8(), face(entry.state)));
            }
            spans.extend(place.map(|place| (place, Highlight::SessionQuiet)));
            spans.push((age_at, Highlight::SessionQuiet));
            spans
        };
        if spinning {
            out.spinning.push(out.lines.len());
        }
        if blink.is_some() {
            out.blinking.push(out.lines.len());
        }
        if let Some(cells) = named_cells {
            out.bands.lit.push((out.lines.len(), cells));
        }
        out.push_drawn(&cut(&text, width), line.clone(), &spans);
        self.reason(out, width, line);
    }

    /// Its reason, on the line under it, only where the person has something to do.
    fn reason(&self, out: &mut Rendered, width: usize, line: Line) {
        let entry = self.entry;
        let own = matches!(entry.state, State::NeedsInput(_) | State::Failed)
            .then_some(entry.recap.as_str())
            .filter(|recap| !recap.is_empty());
        let reason = match (self.waiting, own) {
            (0, None) => return,
            (0, Some(own)) => own.to_owned(),
            (1, own) => joined("1 nested needs input", own),
            (n, own) => joined(&format!("{n} nested need input"), own),
        };
        let under = self
            .under
            .map_or_else(|| format!("{}  ", self.indent), str::to_owned);
        let text = cut(&format!("{under}{}", unmarked(&reason)), width);
        // The guides, if any, then the reason, each quiet.
        let guided = under.trim_start().len();
        let quiet = [
            (under.len() - guided..under.len(), Highlight::SessionQuiet),
            (under.len()..text.len(), Highlight::SessionQuiet),
        ];
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

/// `first`, then ` · ` and `then` if there is one.
fn joined(first: &str, then: Option<&str>) -> String {
    then.map_or_else(|| first.to_owned(), |then| format!("{first} · {then}"))
}
