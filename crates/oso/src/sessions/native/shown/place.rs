//! A place in a reply that a line of it is drawn from (ADR 0165, "The look: Markdown"): what the
//! lines before it say of those after, and where to draw again once the reply grew.
//!
//! Must not depend on: the syntax threads, the editor or the theme.

use super::table::{self, Budget};
use super::{MAX_MARKED, MAX_TEXT, deep};

/// The most tables of a reply: what is past them is lines like any.
pub(super) const MAX_TABLES: u16 = 512;

/// A place in a reply where a line of its text starts: how many lines are drawn before it, and
/// what the lines before it say of those after.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineStart {
    /// The byte the line starts at.
    pub from: usize,
    /// The lines drawn before it.
    pub lines: usize,
    pub(super) state: State,
    /// The table that may go on there.
    pub(super) table: Option<Open>,
}

/// A table drawn as written, as more of it may come: where its header starts, and the lines
/// drawn before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Open {
    pub(super) from: usize,
    pub(super) lines: usize,
    pub(super) state: State,
    /// Its line of dashes came.
    pub(super) sized: bool,
}

impl LineStart {
    /// A reply's start, `lines` lines drawn before its first.
    pub fn after(lines: usize) -> Self {
        Self {
            lines,
            ..Self::default()
        }
    }

    /// Whether a table was drawn before it, which takes another width's lines.
    pub fn after_table(self) -> bool {
        self.state.tables > 0
    }

    /// About what the tables before it took to draw, in cells.
    pub fn table_cells(self) -> usize {
        self.state.budget.taken()
    }

    /// Whether the line `shown` that starts here can be a table's header.
    pub(super) fn heads_table(self, shown: &str) -> bool {
        let state = self.state;
        self.table.is_none()
            && table::piped(shown)
                .is_some_and(|indent| state.list || !deep(&shown[..indent]))
            // A quote's ends here.
            && state.fence.is_none_or(|fence| fence.quoted)
            && !state.piped
            && state.tables < MAX_TABLES
            && shown.len() <= MAX_MARKED
            && self.from < MAX_TEXT
    }
}

/// Where to draw `text` again from once it grew past `mark`, or ended unless it `streams`: there,
/// or where the table that was open there starts, unless it goes on.
pub fn again(text: &str, mark: LineStart, streams: bool) -> LineStart {
    let goes_on = |open: Open| {
        // Nothing ended, or no line but its own.
        let rest = &text[mark.from..];
        streams && (!rest.contains('\n') || (open.sized && table::goes_on(rest)))
    };
    match mark.table {
        Some(open) if !goes_on(open) => LineStart {
            from: open.from,
            lines: open.lines,
            state: open.state,
            table: None,
        },
        _ => mark,
    }
}

/// What the lines so far say of the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct State {
    /// The fence that is open.
    pub(super) fence: Option<Fence>,
    /// The line before has text.
    pub(super) text: bool,
    /// A list's item came, and nothing at the margin since.
    pub(super) list: bool,
    /// The lines before are a block's indented code.
    pub(super) code: bool,
    /// A hidden fence ended just before.
    pub(super) ended: bool,
    /// The line before starts with a `|`, so this one heads no table.
    pub(super) piped: bool,
    /// The rules drawn so far, and the tables.
    pub(super) rules: u8,
    pub(super) tables: u16,
    /// What its tables may still take.
    pub(super) budget: Budget,
}

/// An open fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Fence {
    pub(super) tilde: bool,
    pub(super) len: usize,
    pub(super) indent: usize,
    /// Its two lines are drawn.
    pub(super) drawn: bool,
    /// Its lines are a quote's.
    pub(super) quoted: bool,
    /// The byte its first line starts at.
    pub(super) start: usize,
}
