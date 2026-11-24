//! The text of a request's line (ADR 0182): the keys that act on it, each on a chip before what it
//! does, and at the right what of it was read.
//!
//! Must not depend on: the app or the UI; plain functions over a block and what it was read.
//!
//! - Oso's words and the tool's name only, never a target, a path or a reason of Claude's.
//! - What it shows is what a key does now: the keys are dim while an answer would be refused, and
//!   `y` and `C-u y` while what they need was not read. Whether one answers is not read from here:
//!   the gate asks [`Reading::allows`] and [`Reading::may_ask`], as this does.
//! - Where the keys do not fit a row they take fewer words, then more rows: none is left out.

use oso_core::minibuffer::cells_of;

use super::Block;
use super::body::Hidden;
use super::reading::Reading;
use super::title::{GAP, cut, said};
use crate::sessions::native::render::{Face, Rendered};

/// The narrowest window a line takes an answer in, in cells.
pub const NARROWEST: usize = 20;
/// The most cells of the tool's name at the right.
const MAX_NAME: usize = 24;

/// What a line says, beside its block and what of it was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Says {
    /// Point is on the line.
    pub here: bool,
    /// An answer would be taken now, but for what was not read: the wait is over, no macro runs,
    /// and oso's window is in front.
    pub answers: bool,
}

/// Which keys a line shows, by what they do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keys {
    /// Going to the line, as point is not there.
    Go,
    /// All of it was read, or will be once it was in view long enough: `y` allows.
    Allow,
    /// All of it was read, and its end is out of view: `SPC` goes on to it.
    ToEnd,
    /// Some of it was not read, and is out of view.
    ReadOn,
    /// What was not read is not shown.
    Unread,
    /// None: the window is too narrow.
    Narrow,
}

/// A line as text and faces, its rows apart by a line break, and which keys it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub shown: Rendered,
    pub rows: usize,
    pub keys: Keys,
}

/// A key, what it does, and that in fewer words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Allow,
    Unread,
    ReadOn,
    Deny,
    Why,
    /// `N`, always in few words.
    WhyShort,
    Input,
    Write,
    Go,
}

impl Key {
    fn words(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Allow => ("y", "allow", "allow"),
            Self::Unread => ("C-u y", "unread", "unread"),
            Self::ReadOn => ("SPC", "read on", "read on"),
            Self::Deny => ("n", "deny", "deny"),
            Self::Why => ("N", "deny, saying why", "why"),
            Self::WhyShort => ("N", "why", "why"),
            Self::Input => ("RET", "its input", "input"),
            Self::Write => ("i", "write", "write"),
            Self::Go => ("C-c C-j", "answers it", "answers"),
        }
    }
}

/// The states a line at which point is can be in.
const HERE: [Keys; 4] = [Keys::Allow, Keys::ToEnd, Keys::ReadOn, Keys::Unread];

impl Keys {
    /// The keys shown of a request read as `reading` says.
    fn of(says: Says, reading: &Reading, width: usize) -> Self {
        if width < NARROWEST {
            return Self::Narrow;
        }
        if !says.here {
            return Self::Go;
        }
        let soon = reading.shown_read() || reading.unread_in_view();
        match (reading.hidden(), soon) {
            (_, false) => Self::ReadOn,
            (Hidden::Nothing, true) if reading.last_in_view() => Self::Allow,
            (Hidden::Nothing, true) => Self::ToEnd,
            (Hidden::Lines(_) | Hidden::All, true) => Self::Unread,
        }
    }

    fn keys(self) -> &'static [Key] {
        match self {
            Self::Go => &[Key::Go],
            Self::Allow => &[Key::Allow, Key::Deny, Key::Why, Key::Input, Key::Write],
            Self::ToEnd => &[Key::ReadOn, Key::Deny, Key::Why, Key::Input, Key::Write],
            Self::ReadOn => &[
                Key::ReadOn,
                Key::Deny,
                Key::WhyShort,
                Key::Unread,
                Key::Write,
            ],
            Self::Unread => &[Key::Unread, Key::Deny, Key::Why, Key::Input, Key::Write],
            Self::Narrow => &[],
        }
    }
}

/// The cells a line keeps free at each end, the cursor's among them.
const MARGIN: usize = 1;

/// `block`'s line, `width` cells wide, of which `reading` was read. Where point is on it, it has as
/// many rows at a width whatever it says, so that what it says never moves what it is said of.
pub fn line(block: &Block, reading: &Reading, says: Says, width: usize) -> Line {
    let keys = Keys::of(says, reading, width);
    let margin = if keys == Keys::Narrow { 0 } else { MARGIN };
    let width = width - 2 * margin;
    let rows = match keys {
        Keys::Narrow => vec![vec![(said(block, "", width), Face::CardTitle)]],
        Keys::Go => go(block, width),
        _ => {
            // What a key's answer needs to have been read.
            let lit = |key| {
                says.answers
                    && match key {
                        Key::Allow => reading.allows(),
                        Key::Unread => reading.may_ask(),
                        _ => true,
                    }
            };
            let (_, of) = reading.lines();
            let widest = widest(block, of, width);
            let mut rows = chips(keys, widest, lit, width);
            let most = HERE.map(|keys| chips(keys, widest, |_| true, width).len());
            let most = most.into_iter().max().unwrap_or(1);
            rows.resize(rows.len().max(most), Pieces::new());
            if let Some(last) = rows.last_mut() {
                let right = right(block, reading, width);
                let before = width.saturating_sub(cells(last) + cells_of(&right));
                last.push((format!("{}{right}", " ".repeat(before)), Face::CardTitle));
            }
            rows
        }
    };
    let mut line = Line {
        rows: rows.len(),
        keys,
        shown: Rendered::default(),
    };
    for (at, row) in rows.iter().enumerate() {
        if at > 0 {
            line.shown.put("\n", Some(Face::CardTitle));
        }
        line.shown.put(&" ".repeat(margin), Some(Face::CardTitle));
        for (text, face) in row {
            line.shown.put(text, Some(*face));
        }
    }
    line
}

/// The cells of the widest of what the line of `block`, of `of` lines, says at its right.
fn widest(block: &Block, of: usize, width: usize) -> usize {
    let read = [
        format!("{} · all read", cut(&block.tool, MAX_NAME)),
        format!("{of} of {of} rows read"),
    ];
    let most = read.iter().map(|words| cells_of(words)).max();
    most.unwrap_or(0).min(width)
}

/// The rows of `keys`, in few words unless one row holds them in more, the last with room at its
/// right for `right` cells, each key dim unless it is `lit`.
fn chips(keys: Keys, right: usize, lit: impl Fn(Key) -> bool, width: usize) -> Vec<Pieces> {
    let fits = |rows: &[Pieces]| {
        let last = rows.last().map_or(0, |row| cells(row));
        last + GAP + right <= width
    };
    let long = rows_of(keys.keys(), false, &lit, width);
    if long.len() == 1 && fits(&long) {
        return long;
    }
    let mut rows = rows_of(keys.keys(), true, &lit, width);
    if !fits(&rows) {
        rows.push(Pieces::new());
    }
    rows
}

/// A row of a line, in pieces each in its face.
type Pieces = Vec<(String, Face)>;

fn cells(row: &[(String, Face)]) -> usize {
    row.iter().map(|(text, _)| cells_of(text)).sum()
}

/// What the line says where point is not: what asks, then how to go there.
fn go(block: &Block, width: usize) -> Vec<Pieces> {
    let words = said(block, "", width);
    let chip = |short| {
        let rows = rows_of(Keys::Go.keys(), short, |_| true, width);
        rows.into_iter().flatten()
    };
    for short in [false, true] {
        let mut row: Pieces = vec![(format!("{words}{}", " ".repeat(GAP)), Face::CardTitle)];
        row.extend(chip(short));
        if cells(&row) <= width {
            return vec![row];
        }
    }
    vec![vec![(words, Face::CardTitle)], chip(false).collect()]
}

/// `keys` as rows of chips at most `width` cells wide, in fewer words if `short`, each dim unless
/// it is `lit`. One that takes more than a row has it to itself, its words cut.
fn rows_of(keys: &[Key], short: bool, lit: impl Fn(Key) -> bool, width: usize) -> Vec<Pieces> {
    let mut rows = vec![Pieces::new()];
    for key in keys {
        let (on_chip, words) = match lit(*key) {
            true => (Face::CardKey, Face::CardTitle),
            false => (Face::Dim, Face::Dim),
        };
        let (key, does, shortly) = key.words();
        let key = format!(" {key} ");
        let does = format!(" {}", if short { shortly } else { does });
        let does = cut(&does, width.saturating_sub(cells_of(&key)));
        let chip = [(key, on_chip), (does, words)];
        let used = rows.last().map_or(0, |row| cells(row));
        if used > 0 && used + GAP + cells(&chip) > width {
            rows.push(Pieces::new());
        }
        let Some(row) = rows.last_mut() else { continue };
        if !row.is_empty() {
            row.push((" ".repeat(GAP), Face::CardTitle));
        }
        row.extend(chip);
    }
    rows
}

/// What the line says at its right: the tool, and what of its call was read.
fn right(block: &Block, reading: &Reading, width: usize) -> String {
    let tool = cut(&block.tool, MAX_NAME);
    let (read, of) = reading.lines();
    let words = match reading.hidden() {
        Hidden::All => "not shown at all".to_owned(),
        _ if reading.all_read() => format!("{tool} · all read"),
        Hidden::Lines(lines) if reading.shown_read() => {
            format!("{} not shown", super::body::lines(lines))
        }
        // What is in view is about to be read.
        Hidden::Nothing if reading.unread_in_view() => tool,
        Hidden::Nothing | Hidden::Lines(_) => format!("{read} of {of} rows read"),
    };
    cut(&words, width)
}

#[cfg(test)]
mod tests;
