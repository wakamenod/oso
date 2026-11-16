//! The bands of `*Spaces*` in the side window (ADR 0140): a section's row on its band, the
//! front worktree's rows on their tint, and, only while the pane is selected, the row at point
//! on the point's band with a bar in its first cell. The pane not selected draws no band for
//! point, so that whether it has the person's keys shows.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::buffer::Mode;
use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::window::{Screen, WindowId};

use crate::sessions::list::SpaceBands;

/// What the side window fills behind the list's text, kept from frame to frame.
#[derive(Default)]
pub(super) struct SpacesFills {
    pub(super) sections: Vec<CellRect>,
    pub(super) tint: Vec<CellRect>,
    pub(super) point: Vec<CellRect>,
    /// The first cell of the row at point, where the bar goes.
    pub(super) bar: Vec<CellRect>,
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
        self.point.clear();
        self.bar.clear();
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
    }

    #[test]
    fn not_selected_no_band_marks_point_but_the_sections_stay() {
        let (mut editor, side, bands) = shown(2);
        editor.other_window(1);
        let (fills, band) = filled(&editor, side, &bands);
        assert!(band.is_empty() && fills.point.is_empty() && fills.bar.is_empty());
        assert_eq!(fills.sections.len(), 1);
        // Another window draws none of the pane's bands.
        let tree = editor.window().id();
        let (fills, _) = filled(&editor, tree, &bands);
        assert!(fills.sections.is_empty() && fills.tint.is_empty());
    }
}
