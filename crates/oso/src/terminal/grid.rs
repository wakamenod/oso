//! What the UI keeps of a terminal's screen (ADR 0120, Q4): every visible row, as oso-render
//! draws it, merged from the rows each snapshot says changed. A row gets a new generation when
//! it changes, so the renderer shapes only rows that did. The mouse's selection is kept here
//! too (step 4b-2): its cells are drawn `SELECTED`, it follows its text while output scrolls,
//! and output into it, or a new size, clears it.
//!
//! Must not depend on: `app` or the terminal's thread. A snapshot is read here and given back
//! by the caller.

use std::ops::Range;

use oso_platform::process::pty::Size;
use oso_render::{GridCell, GridCursor, GridCursorShape, GridRow, GridStyle};
use oso_term::{Cursor, CursorShape, Modes, Row, Snapshot};

use super::colors;
use super::selection::{Selection, Spot};

/// One kept row.
#[derive(Default)]
struct Kept {
    generation: u64,
    cells: Vec<GridCell>,
    text: String,
    graphemes: Vec<(u16, Range<u32>)>,
    /// The row goes on in the next, soft-wrapped.
    wrapped: bool,
    /// The columns its cells are marked selected in, and those cells' styles unmarked.
    selected: Option<Range<u16>>,
    unmarked: Vec<GridStyle>,
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
    selection: Option<Selection>,
    /// A row's text, read again into the same string while a selection is checked.
    scratch: String,
}

impl Grid {
    /// Merges `snapshot`'s changed rows, cursor and modes in. A selection follows its text
    /// as output scrolls it, and is cleared by a new size or by output into a selected line.
    pub(crate) fn apply(&mut self, snapshot: &Snapshot) {
        let rows = usize::from(snapshot.size.rows);
        if self.size != Some(snapshot.size) {
            self.rows.truncate(rows);
            self.rows.resize_with(rows, Kept::default);
            self.size = Some(snapshot.size);
            // Rows the snapshot does not send again lose their marks too.
            if self.selection.take().is_some() {
                self.mark();
            }
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
        if self.selection.is_some() {
            if self.written_into(snapshot) {
                self.selection = None;
            } else {
                self.note_selected_lines();
            }
            self.mark();
        }
    }

    /// Whether `snapshot` changed the text of a selected line on the screen. Only rows it
    /// sent, whose line is selected and known, are read, into one scratch string.
    fn written_into(&mut self, snapshot: &Snapshot) -> bool {
        let top = self.top();
        let Some(selection) = &self.selection else {
            return false;
        };
        snapshot.rows.iter().any(|row| {
            let index = usize::from(row.index);
            let line = top + index;
            let Some(kept) = self.rows.get(index).filter(|_| selection.knows(line)) else {
                return false;
            };
            row_text_into(kept, 0..usize::MAX, Reading::WHOLE, &mut self.scratch);
            selection.changed(line, &self.scratch)
        })
    }

    /// Keeps each selected line on the screen as it is now, the first time it is seen, and
    /// forgets the lines off it: the selection costs the rows shown, whatever its length.
    fn note_selected_lines(&mut self) {
        let shown = self.shown();
        let Some(selection) = &mut self.selection else {
            return;
        };
        let lines = selection.lines();
        let (first, last) = (
            (*lines.start()).max(shown.start),
            (*lines.end()).min(shown.end),
        );
        for line in first..last.saturating_add(1).min(shown.end) {
            if !selection.knows(line)
                && let Some(kept) = self.rows.get(line - shown.start)
            {
                row_text_into(kept, 0..usize::MAX, Reading::WHOLE, &mut self.scratch);
                selection.note(line, &self.scratch);
            }
        }
        selection.keep(&shown);
    }

    /// The lines of the scrollback the screen shows.
    fn shown(&self) -> Range<usize> {
        let top = self.top();
        top..top + self.rows.len()
    }

    /// The line of the scrollback the screen's top row shows.
    fn top(&self) -> usize {
        self.history.saturating_sub(self.scrolled)
    }

    /// Begins a selection at the screen's cell `row`, `column`, in place of any; it holds no
    /// cell until [`Grid::select_to`]. Returns whether a selection drawn went.
    pub(crate) fn select_from(&mut self, row: u16, column: u16) -> bool {
        let at = Spot {
            line: self.top() + usize::from(row),
            column,
        };
        self.selection = Some(Selection::new(at));
        self.mark()
    }

    /// Moves the selection's end to the screen's cell `row`, `column`. Returns whether that
    /// changed what is drawn.
    pub(crate) fn select_to(&mut self, row: u16, column: u16) -> bool {
        let top = self.top();
        let Some(selection) = &mut self.selection else {
            return false;
        };
        let to = Spot {
            line: top + usize::from(row),
            column,
        };
        if !selection.extend(to) {
            return false;
        }
        self.note_selected_lines();
        self.mark()
    }

    /// Drops the selection. Returns whether one was drawn.
    pub(crate) fn clear_selection(&mut self) -> bool {
        self.selection.take().is_some() && self.mark()
    }

    /// The selected cells' text: rows joined by line breaks, but for a row that soft-wraps
    /// into the next, and each row's trailing blanks dropped unless it wraps. Lines scrolled
    /// out of sight are left out. `None` with nothing selected.
    pub(crate) fn selection_text(&self) -> Option<String> {
        let selection = self.selection.as_ref().filter(|s| !s.is_empty())?;
        let (top, width) = (self.top(), self.width());
        let (lines, shown) = (selection.lines(), self.shown());
        let last = *lines.end();
        let mut text = String::new();
        for line in (*lines.start()).max(shown.start)..=last.min(shown.end.saturating_sub(1)) {
            let Some(kept) = self.rows.get(line - top) else {
                continue;
            };
            if let Some(columns) = selection.columns(line, width) {
                let columns = usize::from(columns.start)..usize::from(columns.end);
                let mut piece = String::new();
                let reading = Reading {
                    trim: !kept.wrapped,
                    conceal: true,
                };
                row_text_into(kept, columns, reading, &mut piece);
                text.push_str(&piece);
            }
            if line != last && !kept.wrapped {
                text.push('\n');
            }
        }
        Some(text)
    }

    /// How many lines' texts the selection keeps, for the tests of its cost.
    #[cfg(test)]
    pub(crate) fn selection_known(&self) -> usize {
        self.selection.as_ref().map_or(0, Selection::known)
    }

    /// The screen's width in cells.
    fn width(&self) -> u16 {
        self.size.map_or(0, |size| size.columns)
    }

    /// Marks each row's cells as the selection holds them, giving a row whose marks changed
    /// a new generation. Returns whether any did.
    fn mark(&mut self) -> bool {
        let (top, width) = (self.top(), self.width());
        let mut changed = false;
        for (index, kept) in self.rows.iter_mut().enumerate() {
            let wanted = (self.selection.as_ref()).and_then(|s| s.columns(top + index, width));
            if kept.selected == wanted {
                continue;
            }
            // The styles the marks covered come back as they were.
            let columns = |range: &Option<Range<u16>>| {
                range
                    .as_ref()
                    .map_or(0..0, |r| usize::from(r.start)..usize::from(r.end))
            };
            let old = columns(&kept.selected);
            let cells = kept.cells.get_mut(old).into_iter().flatten();
            for (cell, style) in cells.zip(kept.unmarked.drain(..)) {
                cell.style = style;
            }
            kept.unmarked.clear();
            let new = columns(&wanted);
            for cell in kept.cells.get_mut(new).into_iter().flatten() {
                kept.unmarked.push(cell.style);
                cell.style = cell.style.with(GridStyle::SELECTED);
            }
            kept.selected = wanted;
            self.next += 1;
            kept.generation = self.next;
            changed = true;
        }
        changed
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

    pub(crate) fn scrolled(&self) -> usize {
        self.scrolled
    }

    pub(crate) fn history(&self) -> usize {
        self.history
    }

    /// How many rows the screen has.
    pub(crate) fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Whether row `index` goes on in the next, soft-wrapped.
    pub(crate) fn row_wraps(&self, index: usize) -> bool {
        self.rows.get(index).is_some_and(|kept| kept.wrapped)
    }

    pub(crate) fn size(&self) -> Option<Size> {
        self.size
    }

    /// The text of row `index`, trailing blanks dropped, for the dev socket's state and the
    /// agent channel's `text` where the person allowed it (ADR 0120, Q16).
    /// One pass over the cells and the clusters, which come in column order, so a row of
    /// clusters costs its length and not its length times their count.
    pub(crate) fn row_text(&self, index: usize) -> Option<String> {
        let mut text = String::new();
        row_text_into(
            self.rows.get(index)?,
            0..usize::MAX,
            Reading::WHOLE,
            &mut text,
        );
        Some(text)
    }
}

/// How a row's text is read.
#[derive(Clone, Copy)]
struct Reading {
    /// Drop the trailing blanks.
    trim: bool,
    /// Read a concealed cell (SGR 8) as a blank, as it is drawn, so a copy holds only what
    /// the person saw.
    conceal: bool,
}

impl Reading {
    /// Every cell's text as the program wrote it, trailing blanks dropped.
    const WHOLE: Reading = Reading {
        trim: true,
        conceal: false,
    };
}

/// The text of `kept`'s cells in `columns` into `text`, emptied first, read as `reading`
/// says. One pass over the cells and the clusters, which come in column order, so a row of
/// clusters costs its length and not its length times their count. A range that begins on a
/// wide character's second column takes the character.
fn row_text_into(kept: &Kept, columns: Range<usize>, reading: Reading, text: &mut String) {
    let end = columns.end.min(kept.cells.len());
    let mut start = columns.start.min(end);
    if start > 0
        && kept.cells[start..end]
            .first()
            .is_some_and(|cell| cell.style.contains(GridStyle::WIDE_SPACER))
    {
        start -= 1;
    }
    text.clear();
    text.reserve(end - start);
    let mut clusters = kept.graphemes.iter().peekable();
    for (column, cell) in kept.cells[start..end].iter().enumerate() {
        let column = start + column;
        // A cluster before this column was a cell passed over.
        while clusters
            .next_if(|(at, _)| usize::from(*at) < column)
            .is_some()
        {}
        if cell.style.contains(GridStyle::WIDE_SPACER) {
            continue;
        }
        if reading.conceal && cell.style.contains(GridStyle::HIDDEN) {
            clusters.next_if(|(at, _)| usize::from(*at) == column);
            text.push(' ');
            continue;
        }
        match clusters.next_if(|(at, _)| usize::from(*at) == column) {
            Some((_, range)) => text.push_str(
                kept.text
                    .get(range.start as usize..range.end as usize)
                    .unwrap_or_default(),
            ),
            None => text.push(if cell.ch == '\0' { ' ' } else { cell.ch }),
        }
    }
    if reading.trim {
        let kept_len = text.trim_end().len();
        text.truncate(kept_len);
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
    kept.wrapped = row.wrapped;
    // Fresh cells hold no marks; `Grid::mark` puts them back.
    kept.selected = None;
    kept.unmarked.clear();
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
    fn a_rows_text_takes_its_clusters_in_one_pass_and_drops_trailing_blanks() {
        // "e\u{301}" at 0, a wide "日" at 1 with its spacer at 2, "u\u{308}" at 3, blanks after.
        let text = "e\u{301}u\u{308}";
        let mut row = row(0, "e日 u   ");
        row.cells[2].attrs = oso_term::Attrs::WIDE_SPACER;
        row.text = text.to_owned();
        row.graphemes = vec![(0, 0..3), (3, 3..6)];
        let mut grid = Grid::default();
        grid.apply(&snapshot(size(1, 7), vec![row]));
        assert_eq!(grid.row_text(0).as_deref(), Some("e\u{301}日u\u{308}"));
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

#[cfg(test)]
mod selection_tests;
