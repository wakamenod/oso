//! The bands of `*Spaces*` in the side window (ADR 0140): a section's row on its band, the
//! front worktree's rows on their tint, the session shown's on a band of its own, and, only while the pane is selected, the row at point
//! on the point's band with a bar in its first cell. The pane not selected draws no band for
//! point, so that whether it has the person's keys shows. On the lit beat of the blink, a
//! waiting row's mark and name, and a roll-up's mark, are on a fill of their own (ADR 0140, as
//! amended).
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use std::ops::Range;

use oso_core::buffer::Mode;
use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::window::{Screen, ScreenRow, WindowId};

use crate::sessions::list::SpaceBands;

/// What the side window fills behind the list's text, kept from frame to frame.
#[derive(Default)]
pub(super) struct SpacesFills {
    pub(super) sections: Vec<CellRect>,
    pub(super) tint: Vec<CellRect>,
    /// The rows of the session the tree's window shows, whether the pane is selected or not.
    pub(super) shown: Vec<CellRect>,
    pub(super) point: Vec<CellRect>,
    /// The first cell of the row at point, where the bar goes.
    pub(super) bar: Vec<CellRect>,
    /// The cells drawn inverse on the blink's lit beat.
    pub(super) lit: Vec<CellRect>,
}

impl SpacesFills {
    /// Fills in what window `id`, as `screen` shows it, draws behind `*Spaces*` when it is the
    /// side window showing it: its sections' and front worktree's rows, from `bands`, and the
    /// row at point from `band`, the current line's band the editor gave, which it takes over.
    pub(super) fn fill(
        &mut self,
        (editor, bands): (&Editor, &SpaceBands),
        (id, screen): (WindowId, &Screen),
        band: &mut Vec<CellRect>,
    ) {
        self.sections.clear();
        self.tint.clear();
        self.shown.clear();
        self.point.clear();
        self.bar.clear();
        self.lit.clear();
        if editor.side_window() != Some(id) {
            return;
        }
        let shows_list = (editor.window_by_id(id))
            .and_then(|window| editor.buffer(window.buffer()))
            .is_some_and(|buffer| buffer.mode() == Mode::Sessions);
        let regions = editor.frame().windows().iter().find(|(w, _)| *w == id);
        let (true, Some((_, regions))) = (shows_list, regions) else {
            return;
        };
        let text = regions.text;
        for (index, row) in screen.rows().iter().take(text.rows).enumerate() {
            let at = text.row + index;
            if bands.sections.binary_search(&row.line).is_ok() {
                push_row(&mut self.sections, text, at);
            }
            if bands.tint.binary_search(&row.line).is_ok() {
                push_row(&mut self.tint, text, at);
            }
            if bands.shown.binary_search(&row.line).is_ok() {
                push_row(&mut self.shown, text, at);
            }
            push_lit(&mut self.lit, &bands.lit, (row, at), text);
        }
        if editor.tiled().0.id() == id {
            let first = band.first().map(|rect| CellRect {
                columns: 1,
                ..*rect
            });
            self.bar.extend(first);
            self.point.append(band);
        }
        band.clear();
    }
}

/// Adds the cells of `lit` that `row`, at frame row `at` of the text area `text`, shows to
/// `rects`: by the cells of its line from where the row starts, as the display model gives
/// them, though the side window cuts a long line rather than wrap it.
fn push_lit(
    rects: &mut Vec<CellRect>,
    lit: &[(usize, Range<usize>)],
    (row, at): (&ScreenRow, usize),
    text: CellRect,
) {
    let from = lit.partition_point(|(line, _)| *line < row.line);
    for (_, columns) in lit[from..].iter().take_while(|(line, _)| *line == row.line) {
        let start = columns.start.max(row.column) - row.column;
        let end = (columns.end.min(row.end_column).saturating_sub(row.column)).min(text.columns);
        if start < end {
            rects.push(CellRect {
                row: at,
                rows: 1,
                column: text.column + start,
                columns: end - start,
            });
        }
    }
}

/// Adds row `at` of the text area `text` to `rects`, as one more row of the last rectangle
/// when it is the row under it.
fn push_row(rects: &mut Vec<CellRect>, text: CellRect, at: usize) {
    match rects.last_mut() {
        Some(last) if last.row + last.rows == at => last.rows += 1,
        _ => rects.push(CellRect {
            row: at,
            rows: 1,
            ..text
        }),
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::{Buffer, Mode};
    use oso_core::frame::FrameSize;

    use super::*;

    /// An editor with a file in the tree and `*Spaces*` in the side window, point on its line
    /// `at`, and its bands: line 1 a section's, line 3 the front's.
    fn shown(at: usize) -> (Editor, WindowId, SpaceBands) {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", "one\n"));
        editor.set_frame_size(FrameSize {
            rows: 24,
            columns: 100,
        });
        let list = editor.add_internal_buffer("*Spaces*", Mode::Sessions);
        editor.set_internal_text(list, " Spaces\n ▾ oso ─ ⚠ 1\n   ▾ main\n     └ ⚠ x\n");
        let side = editor.show_side_window(list, 44);
        editor.select_window(side);
        let offset = editor.current().1.text().line_start(at);
        editor.current_mut().0.selections = oso_core::selection::SelectionSet::new(offset);
        let bands = SpaceBands {
            sections: vec![1],
            tint: vec![3],
            shown: vec![3],
            lit: vec![(1, 14..17), (3, 6..9)],
        };
        (editor, side, bands)
    }

    fn filled(editor: &Editor, id: WindowId, bands: &SpaceBands) -> (SpacesFills, Vec<CellRect>) {
        let mut screen = Screen::default();
        editor.screen_of(id, &mut screen);
        let mut band: Vec<CellRect> = editor.current_line_cells(id, &screen).into_iter().collect();
        let mut fills = SpacesFills::default();
        fills.fill((editor, bands), (id, &screen), &mut band);
        (fills, band)
    }

    #[test]
    fn selected_the_row_at_point_takes_the_accent_band_and_the_bar() {
        let (editor, side, bands) = shown(2);
        let (fills, band) = filled(&editor, side, &bands);
        assert!(band.is_empty(), "the current line's band gives way");
        assert_eq!(fills.point.len(), 1);
        assert_eq!(fills.point[0].row, 2);
        assert_eq!(fills.bar.len(), 1);
        assert_eq!((fills.bar[0].row, fills.bar[0].columns), (2, 1));
        assert_eq!(
            fills.sections.iter().map(|r| r.row).collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(fills.tint.iter().map(|r| r.row).collect::<Vec<_>>(), [3]);
        assert_eq!(fills.shown.iter().map(|r| r.row).collect::<Vec<_>>(), [3]);
    }

    #[test]
    fn the_lit_beat_fills_its_cells_on_their_rows_cut_to_the_window() {
        let (mut editor, side, mut bands) = shown(2);
        let (fills, _) = filled(&editor, side, &bands);
        let text = editor
            .frame()
            .windows()
            .iter()
            .find(|(w, _)| *w == side)
            .unwrap()
            .1
            .text;
        let lit: Vec<_> = (fills.lit.iter())
            .map(|r| (r.row - text.row, r.column - text.column, r.columns, r.rows))
            .collect();
        assert_eq!(lit, [(1, 14, 3, 1), (3, 6, 3, 1)]);
        // Past the window's edge, it is cut; wholly past it, nothing.
        bands.lit = vec![
            (1, text.columns - 1..text.columns + 4),
            (3, text.columns..text.columns + 2),
        ];
        let (fills, _) = filled(&editor, side, &bands);
        assert_eq!(fills.lit.len(), 1);
        assert_eq!(fills.lit[0].columns, 1);
        // Another window draws none.
        editor.other_window(1);
        let tree = editor.window().id();
        let (fills, _) = filled(&editor, tree, &bands);
        assert!(fills.lit.is_empty());
    }

    #[test]
    fn not_selected_no_band_marks_point_but_the_sections_stay() {
        let (mut editor, side, bands) = shown(2);
        editor.other_window(1);
        let (fills, band) = filled(&editor, side, &bands);
        assert!(band.is_empty() && fills.point.is_empty() && fills.bar.is_empty());
        assert_eq!(fills.sections.len(), 1);
        // The session shown's band shows whether the pane is selected or not (ADR 0140).
        assert_eq!(fills.shown.len(), 1);
        // Another window draws none of the pane's bands.
        let tree = editor.window().id();
        let (fills, _) = filled(&editor, tree, &bands);
        assert!(fills.sections.is_empty() && fills.tint.is_empty() && fills.shown.is_empty());
    }
}
