//! What the UI keeps of a terminal's screen (ADR 0120, Q4): every visible row, as oso-render
//! draws it, merged from the rows each snapshot says changed. A row gets a new generation when
//! it changes, so the renderer shapes only rows that did.
//!
//! Must not depend on: `app` or the terminal's thread. A snapshot is read here and given back
//! by the caller.

use std::ops::Range;

use oso_platform::process::pty::Size;
use oso_render::{GridCell, GridCursor, GridCursorShape, GridRow};
use oso_term::{Cursor, CursorShape, Modes, Row, Snapshot};

use super::colors;

/// One kept row.
#[derive(Default)]
struct Kept {
    generation: u64,
    cells: Vec<GridCell>,
    text: String,
    graphemes: Vec<(u16, Range<u32>)>,
}

/// A terminal's screen as the UI last took it.
#[derive(Default)]
pub(crate) struct Grid {
    rows: Vec<Kept>,
    size: Option<Size>,
    /// The generation the next changed row takes.
    next: u64,
    cursor: Cursor,
    modes: Modes,
    /// Lines the view is scrolled back from the bottom.
    scrolled: usize,
    /// Lines in the scrollback.
    history: usize,
}

impl Grid {
    /// Merges `snapshot`'s changed rows, cursor and modes in.
    pub(crate) fn apply(&mut self, snapshot: &Snapshot) {
        let rows = usize::from(snapshot.size.rows);
        if self.size != Some(snapshot.size) {
            self.rows.truncate(rows);
            self.rows.resize_with(rows, Kept::default);
            self.size = Some(snapshot.size);
        }
        for row in &snapshot.rows {
            if let Some(kept) = self.rows.get_mut(usize::from(row.index)) {
                self.next += 1;
                fill(kept, row, self.next);
            }
        }
        self.cursor = snapshot.cursor;
        self.modes = snapshot.modes;
        self.scrolled = snapshot.scrolled;
        self.history = snapshot.history;
    }

    /// Every row from the top, as oso-render takes them, into `out`.
    pub(crate) fn rows_into<'a>(&'a self, out: &mut Vec<GridRow<'a>>) {
        out.extend(self.rows.iter().map(|kept| GridRow {
            generation: kept.generation,
            cells: &kept.cells,
            text: &kept.text,
            graphemes: &kept.graphemes,
        }));
    }

    /// The cursor as drawn: none while the program hides it, hollow in a window that is not
    /// selected, or when the program asked for a hollow box.
    pub(crate) fn cursor(&self, selected: bool) -> Option<GridCursor> {
        if !self.cursor.visible || self.scrolled > 0 {
            return None;
        }
        let shape = match self.cursor.shape {
            CursorShape::Block | CursorShape::HollowBlock => GridCursorShape::Block,
            CursorShape::Bar => GridCursorShape::Bar,
            CursorShape::Underline => GridCursorShape::Underline,
        };
        Some(GridCursor {
            row: self.cursor.row,
            column: self.cursor.column,
            shape,
            hollow: !selected || self.cursor.shape == CursorShape::HollowBlock,
        })
    }

    /// The cursor's cell, whether the program shows it or not, for the input method's
    /// preedit and candidate window (ADR 0120, Input).
    pub(crate) fn cursor_cell(&self) -> (u16, u16) {
        (self.cursor.row, self.cursor.column)
    }

    pub(crate) fn modes(&self) -> Modes {
        self.modes
    }

    #[cfg(feature = "dev")]
    pub(crate) fn scrolled(&self) -> usize {
        self.scrolled
    }

    #[cfg(feature = "dev")]
    pub(crate) fn history(&self) -> usize {
        self.history
    }

    #[cfg(feature = "dev")]
    pub(crate) fn size(&self) -> Option<Size> {
        self.size
    }

    /// The text of row `index`, trailing blanks dropped, for the dev socket's state.
    #[cfg(feature = "dev")]
    pub(crate) fn row_text(&self, index: usize) -> Option<String> {
        let kept = self.rows.get(index)?;
        let mut text = String::new();
        for (column, cell) in kept.cells.iter().enumerate() {
            if cell.style.contains(oso_render::GridStyle::WIDE_SPACER) {
                continue;
            }
            match kept
                .graphemes
                .iter()
                .find(|(at, _)| usize::from(*at) == column)
            {
                Some((_, range)) => text.push_str(
                    kept.text
                        .get(range.start as usize..range.end as usize)
                        .unwrap_or_default(),
                ),
                None => text.push(if cell.ch == '\0' { ' ' } else { cell.ch }),
            }
        }
        Some(text.trim_end().to_owned())
    }
}

/// Puts `row` into `kept`, keeping its allocations.
fn fill(kept: &mut Kept, row: &Row, generation: u64) {
    kept.generation = generation;
    kept.cells.clear();
    kept.cells.extend(row.cells.iter().map(|cell| GridCell {
        ch: cell.ch,
        fg: colors::color(cell.fg),
        bg: colors::color(cell.bg),
        underline_color: colors::color(cell.underline_color),
        style: colors::style(cell),
    }));
    kept.text.clear();
    kept.text.push_str(&row.text);
    kept.graphemes.clear();
    kept.graphemes.extend(row.graphemes.iter().cloned());
}

#[cfg(test)]
mod tests {
    use oso_platform::process::pty::Size;
    use oso_render::{GridColor, GridCursorShape, GridRow};
    use oso_term::{Cell, Color, Cursor, CursorShape, Modes, Row, Snapshot};

    use super::Grid;

    fn size(rows: u16, columns: u16) -> Size {
        Size {
            rows,
            columns,
            width: 0,
            height: 0,
        }
    }

    fn row(index: u16, text: &str) -> Row {
        Row {
            index,
            cells: text
                .chars()
                .map(|ch| Cell {
                    ch,
                    fg: Color::Indexed(1),
                    ..Cell::default()
                })
                .collect(),
            ..Row::default()
        }
    }

    fn snapshot(size: Size, rows: Vec<Row>) -> Snapshot {
        Snapshot {
            size,
            rows,
            cursor: Cursor {
                row: 1,
                column: 2,
                visible: true,
                blinking: false,
                shape: CursorShape::Bar,
            },
            modes: Modes::default(),
            echo_off: false,
            history: 0,
            scrolled: 0,
            events: Vec::new(),
            dropped_events: 0,
        }
    }

    fn generations(grid: &Grid) -> Vec<u64> {
        let mut rows: Vec<GridRow<'_>> = Vec::new();
        grid.rows_into(&mut rows);
        rows.iter().map(|row| row.generation).collect()
    }

    #[test]
    fn only_rows_that_changed_get_a_new_generation() {
        let mut grid = Grid::default();
        grid.apply(&snapshot(
            size(3, 4),
            vec![row(0, "ab"), row(1, "cd"), row(2, "")],
        ));
        let first = generations(&grid);
        assert_eq!(first.len(), 3);
        grid.apply(&snapshot(size(3, 4), vec![row(1, "xy")]));
        let second = generations(&grid);
        assert_eq!(second[0], first[0]);
        assert_ne!(second[1], first[1]);
        assert_eq!(second[2], first[2]);
        let mut rows = Vec::new();
        grid.rows_into(&mut rows);
        assert_eq!(rows[1].cells[0].ch, 'x');
        assert_eq!(
            rows[1].cells[0].fg,
            GridColor::Face(oso_render::Face::TerminalColor1)
        );
    }

    #[test]
    fn a_new_size_keeps_as_many_rows() {
        let mut grid = Grid::default();
        grid.apply(&snapshot(size(3, 4), vec![row(0, "a")]));
        grid.apply(&snapshot(size(2, 4), vec![row(0, "b"), row(1, "c")]));
        assert_eq!(generations(&grid).len(), 2);
        // A row past the screen is ignored.
        grid.apply(&snapshot(size(2, 4), vec![row(5, "z")]));
        assert_eq!(generations(&grid).len(), 2);
    }

    #[test]
    fn the_cursor_is_the_programs_hollow_elsewhere_and_hidden_when_scrolled_back() {
        let mut grid = Grid::default();
        let mut taken = snapshot(size(3, 4), Vec::new());
        grid.apply(&taken);
        let cursor = grid.cursor(true).unwrap();
        assert_eq!((cursor.row, cursor.column), (1, 2));
        assert_eq!(cursor.shape, GridCursorShape::Bar);
        assert!(!cursor.hollow);
        assert!(grid.cursor(false).unwrap().hollow);
        taken.scrolled = 3;
        grid.apply(&taken);
        assert!(grid.cursor(true).is_none());
        taken.scrolled = 0;
        taken.cursor.visible = false;
        grid.apply(&taken);
        assert!(grid.cursor(true).is_none());
        // The input method still has its cell.
        assert_eq!(grid.cursor_cell(), (1, 2));
    }
}
