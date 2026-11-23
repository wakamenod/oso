//! How far the rows a line goes on in are indented, in a native session's transcript (ADR 0165).
//! They line up under the line's text, past its spaces and the leaders it starts with.
//!
//! Must not depend on: the tree of rows, windows or the renderer. It reads the start of one
//! logical line, at most half a row of it.

use std::iter::Peekable;

use super::fit::Fit;
use crate::display_map::{DisplayCluster, DisplayPoint, DisplaySnapshot};

/// The least indent of a row that goes on, whatever its line starts with.
const LEAST: usize = 2;

/// The cells the widest display cluster takes but a tab, a boxed label (ADR 0104), which an
/// indented row still holds.
const WIDEST: usize = 8;

/// How many leaders in a row a line's text starts after: `│ ▸ ≡ `.
const LEADERS: usize = 4;

/// The most digits of a numbered item's leader.
const DIGITS: usize = 9;

impl Fit {
    /// The last column whose cluster [`Fit::indent`] reads: an edit at it or before it may
    /// change the indent of its line. `None` where no row is indented.
    pub(super) fn reach(self) -> Option<usize> {
        (self.indents && self.wraps()).then(|| self.most())
    }

    /// The most cells a row is indented by: half the width, and what leaves room for the
    /// widest cluster.
    fn most(self) -> usize {
        (self.width / 2).min(self.width.saturating_sub(WIDEST))
    }

    /// The cells the rows that logical line `line` of `display` goes on in are indented by:
    /// its leading spaces, then up to [`LEADERS`] leaders with the spaces after each, at least
    /// [`LEAST`] and at most [`Fit::most`]. 0 where rows are not indented.
    pub(super) fn indent(self, display: &DisplaySnapshot, line: usize) -> usize {
        let Some(most) = self.reach() else {
            return 0;
        };
        let mut clusters = display
            .clusters(DisplayPoint::new(line, 0))
            .take_while(|cluster| cluster.column <= most)
            .peekable();
        let mut indent = spaces(&mut clusters).unwrap_or(0);
        for _ in 0..LEADERS {
            if !leader(&mut clusters) {
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

/// The indent of one line's rows, read once one is laid out past its first.
pub(super) struct LineIndent<'a> {
    fit: Fit,
    of: Option<(&'a DisplaySnapshot, usize)>,
    known: Option<usize>,
}

impl<'a> LineIndent<'a> {
    /// The indent of logical line `line` of `display`.
    pub(super) fn of(fit: Fit, display: &'a DisplaySnapshot, line: usize) -> Self {
        Self {
            fit,
            of: Some((display, line)),
            known: None,
        }
    }

    /// No indent, at `width` cells a row.
    #[cfg(test)]
    pub(super) fn none(width: usize) -> Self {
        Self {
            fit: Fit {
                width,
                indents: false,
            },
            of: None,
            known: None,
        }
    }

    /// The cells a row holds, its indent among them.
    pub(super) fn width(&self) -> usize {
        self.fit.width.max(1)
    }

    /// The cells of it, less than a row holds.
    pub(super) fn get(&mut self) -> usize {
        let (fit, of) = (self.fit, self.of);
        let read = || of.map_or(0, |(display, line)| fit.indent(display, line));
        *self.known.get_or_insert_with(read)
    }
}

/// Reads the run of U+0020 next in `clusters`, the only space that counts, and says the column
/// after it; `None` if none is next. A space that a character after it joins counts, and ends
/// the run.
fn spaces<'a>(clusters: &mut Peekable<impl Iterator<Item = DisplayCluster<'a>>>) -> Option<usize> {
    let mut end = None;
    while let Some(space) = clusters.next_if(|cluster| cluster.text.starts_with(' ')) {
        end = Some(space.column + 1);
        if space.text != " " {
            break;
        }
    }
    end
}

/// Reads the leader next in `clusters`, if one is: a cluster that starts with neither a letter, a
/// digit nor white space, or at most [`DIGITS`] digits and `.` or `)`.
fn leader<'a>(clusters: &mut Peekable<impl Iterator<Item = DisplayCluster<'a>>>) -> bool {
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
