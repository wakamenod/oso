//! A window's view rows over its buffer as it shows now: its wrap's, or one a logical line
//! (ADR 0100, ADR 0103).
//!
//! Must not depend on: `editor`, `command` or anything that draws.

use crate::display_map::DisplaySnapshot;
use crate::display_map::wrap_map::{WrapMap, WrapSnapshot};
use crate::motion;

/// A window's view rows over its buffer as it shows now: its wrap's, or one a logical line.
#[derive(Clone, Copy)]
pub(crate) struct Rows<'a> {
    display: &'a DisplaySnapshot,
    wrap: Option<&'a WrapSnapshot>,
}

impl<'a> Rows<'a> {
    /// The rows of `wrap` over `display`, or one a logical line without it, or with one
    /// whose text is not `display`'s, as the editor brings it along at the next command.
    pub(crate) fn new(display: &'a DisplaySnapshot, wrap: Option<&'a WrapMap>) -> Self {
        Self {
            display,
            wrap: wrap
                .map(WrapMap::snapshot)
                .filter(|wrap| wrap.len() == display.len()),
        }
    }

    /// How many there are.
    pub(crate) fn count(&self) -> usize {
        match self.wrap {
            Some(wrap) => wrap.row_count(),
            None => self.display.row_count(),
        }
    }

    /// The row that holds `offset`; the last at or past the end.
    pub(crate) fn row_of(&self, offset: usize) -> usize {
        let offset = offset.min(self.display.len());
        match self.wrap {
            Some(wrap) => wrap.row_of(self.display, offset),
            None => self.display.offset_to_display_point(offset).row,
        }
    }

    /// Where row `row` starts, clamped to the last.
    pub(crate) fn start(&self, row: usize) -> usize {
        let row = row.min(self.count() - 1);
        match self.wrap {
            Some(wrap) => wrap.row_span(self.display, row).0,
            None => motion::line_start(self.display, row),
        }
    }

    /// Whether rows wrap.
    pub(crate) fn wrapped(&self) -> bool {
        self.wrap.is_some()
    }

    /// The wrap they come from, if they wrap.
    pub(crate) fn wrap(&self) -> Option<&'a WrapSnapshot> {
        self.wrap
    }
}
