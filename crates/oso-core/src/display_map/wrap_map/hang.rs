//! How far the rows a line goes on in are indented, in a buffer whose rows hang: a native
//! session's transcript (ADR 0165). They line up under the line's text, past its spaces and the
//! marks it starts with.
//!
//! Must not depend on: the tree of rows, windows or the renderer. It reads the start of one
//! logical line, at most half a row of it.

use std::iter::Peekable;

use super::{MIN_COLUMNS, wrap_width};
use crate::display_map::{DisplayCluster, DisplayPoint, DisplaySnapshot};

/// The least indent of a row that goes on, whatever its line starts with.
const LEAST: usize = 2;

/// The cells the widest display cluster takes but a tab, a boxed label (ADR 0104), which an
/// indented row still holds.
const WIDEST: usize = 8;

/// How many marks in a row a line's text starts after: `│ ▸ ≡ `.
const MARKS: usize = 4;

/// The most digits of a numbered item's mark.
const DIGITS: usize = 9;

/// Below this many text columns, a window on a buffer whose rows hang does not wrap: a row
/// then holds an indent of four cells, as a nested row's text takes, with the widest cluster
/// and the mark's column after it.
const HANGING_COLUMNS: usize = MIN_COLUMNS + 3;

/// How a window's rows are laid out: the cells a row holds, and whether the rows a line goes
/// on in are indented.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Fit {
    /// The cells a row holds, the mark's column aside; `usize::MAX` when nothing wraps.
    pub(super) width: usize,
    pub(super) hangs: bool,
}

impl Fit {
    /// The layout in a window `columns` wide.
    pub(super) fn new(columns: usize, hangs: bool) -> Self {
        let width = match hangs && columns < HANGING_COLUMNS {
            true => usize::MAX,
            false => wrap_width(columns),
        };
        Self { width, hangs }
    }

    /// Nothing wraps.
    pub(super) const UNWRAPPED: Self = Self {
        width: usize::MAX,
        hangs: false,
    };

    pub(super) fn wraps(self) -> bool {
        self.width != usize::MAX
    }

    /// The last column whose cluster [`Fit::indent`] reads: an edit at it or before it may
    /// change the indent of its line. `None` where no row is indented.
    pub(super) fn reach(self) -> Option<usize> {
        (self.hangs && self.wraps()).then(|| self.most())
    }

    /// The most cells a row is indented by: half the width, and what leaves room for the
    /// widest cluster.
    fn most(self) -> usize {
        (self.width / 2).min(self.width.saturating_sub(WIDEST))
    }

    /// The cells the rows that logical line `line` of `display` goes on in are indented by:
    /// its leading spaces, then up to [`MARKS`] marks with the spaces after each, at least
    /// [`LEAST`] and at most [`Fit::most`]. 0 where rows do not hang.
    pub(super) fn indent(self, display: &DisplaySnapshot, line: usize) -> usize {
        let Some(most) = self.reach() else {
            return 0;
        };
        let mut clusters = display
            .clusters(DisplayPoint::new(line, 0))
            .take_while(|cluster| cluster.column <= most)
            .peekable();
        let mut indent = spaces(&mut clusters).unwrap_or(0);
        for _ in 0..MARKS {
            if !mark(&mut clusters) {
                break;
            }
            match spaces(&mut clusters) {
                Some(end) => indent = end,
                None => break,
            }
        }
        indent.max(LEAST).min(most)
    }
}

/// Reads the run of U+0020 next in `clusters`, the only space that counts, and says the column
/// after it; `None` if none is next.
fn spaces<'a>(clusters: &mut Peekable<impl Iterator<Item = DisplayCluster<'a>>>) -> Option<usize> {
    let mut end = None;
    while let Some(space) = clusters.next_if(|cluster| cluster.text == " ") {
        end = Some(space.column + space.cells);
    }
    end
}

/// Reads the mark next in `clusters`, if one is: a cluster that starts with neither a letter, a
/// digit nor white space, or at most [`DIGITS`] digits and `.` or `)`.
fn mark<'a>(clusters: &mut Peekable<impl Iterator<Item = DisplayCluster<'a>>>) -> bool {
    let digit = |cluster: &DisplayCluster<'_>| {
        cluster.text.len() == 1 && cluster.text.as_bytes()[0].is_ascii_digit()
    };
    let mut digits = 0;
    while digits < DIGITS && clusters.next_if(digit).is_some() {
        digits += 1;
    }
    let end = clusters.next();
    match digits {
        0 => end
            .and_then(|cluster| cluster.text.chars().next())
            .is_some_and(|first| !first.is_alphanumeric() && !first.is_whitespace()),
        _ => end.is_some_and(|cluster| matches!(cluster.text, "." | ")")),
    }
}

#[cfg(test)]
mod tests;
