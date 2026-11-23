//! How a window's rows are laid out: the cells each holds, and whether the rows a line goes on in
//! are indented.
//!
//! Must not depend on: the tree of rows, windows or the renderer.

use super::MIN_COLUMNS;

/// Below this many text columns, a window on a buffer whose rows are indented does not wrap: a
/// row then holds an indent of six cells, as a call in a nested row takes, with the widest
/// cluster and the mark's column after it (ADR 0165).
pub(super) const INDENTING_COLUMNS: usize = MIN_COLUMNS + 5;

/// How a window's rows are laid out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Fit {
    /// The cells a row holds, its indent among them and the mark's column aside; `usize::MAX`
    /// when nothing wraps.
    pub(super) width: usize,
    /// Whether the rows a line goes on in are indented, by what `indent.rs` says.
    pub(super) indents: bool,
}

impl Fit {
    /// Nothing wraps.
    pub(super) const UNWRAPPED: Self = Self {
        width: usize::MAX,
        indents: false,
    };

    /// The layout in a window `columns` wide.
    pub(super) fn new(columns: usize, indents: bool) -> Self {
        let least = match indents {
            true => INDENTING_COLUMNS,
            false => MIN_COLUMNS,
        };
        let width = match columns < least {
            true => usize::MAX,
            false => columns - 1,
        };
        Self { width, indents }
    }

    pub(super) fn wraps(self) -> bool {
        self.width != usize::MAX
    }
}
