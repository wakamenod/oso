//! The preview beside a float's list in a text-grid snapshot (ADR 0045, ADR 0088, ADR 0114): the
//! previewed buffer's rows or the lines read around a project search's row, its cursor, what is
//! said of what may hold secrets, and the marks of a search's matches.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::editor::{Editor, MatchCells, Preview};
use oso_core::frame;
use oso_core::minibuffer::PromptLayout;
use oso_core::motion;
use oso_core::window::Screen;

use super::rows::{Style, blank_row, draw_row, screen_row};
use super::spans_row;

/// What goes right of the candidates on their row `index` beside a preview: the column that
/// divides them, then the previewed buffer's row from where its window was, or the lines read
/// around a project search's row, its cursor as `o`, or what is said of a buffer that may hold
/// secrets on the first row (ADR 0088, ADR 0114). Also the marks of the preview's cells a
/// search's matches take, `#` for the row's and `~` for the others (ADR 0114).
pub(super) fn preview_row(
    editor: &Editor,
    layout: &PromptLayout,
    index: usize,
) -> (String, Vec<char>) {
    let preview = layout.regions.preview;
    if preview.columns == 0 || index >= preview.rows {
        return (String::new(), Vec::new());
    }
    let mut marks = vec![' '; preview.columns];
    let shown = match editor.preview() {
        Some(Preview::Buffer { id, cursor, .. }) => {
            editor.buffer(id).map(|buffer| (buffer.display(), cursor))
        }
        Some(Preview::Excerpt { cursor, .. }) => editor
            .preview_excerpt_display()
            .map(|display| (display, cursor)),
        _ => None,
    };
    let text = match editor.preview() {
        Some(Preview::Buffer { .. } | Preview::Excerpt { .. }) => {
            let Some((display, cursor)) = shown else {
                return (String::new(), Vec::new());
            };
            let point = display.offset_to_display_point(cursor);
            let mut screen = Screen::default();
            editor.preview_screen(&mut screen);
            let (place, cells) = motion::cursor_box(&display, point, None);
            let cursor = frame::cursor_rect(preview, &screen, place, cells)
                .filter(|cells| cells.row == preview.row + index)
                .map_or(0..0, |cells| {
                    let start = cells.column - preview.column;
                    start..start + cells.columns
                });
            let mut matches = MatchCells::default();
            editor.preview_cells(&screen, &mut matches);
            for (cells, mark) in [(&matches.current, '#'), (&matches.others, '~')] {
                for cells in cells
                    .iter()
                    .filter(|cells| cells.row == preview.row + index)
                {
                    let start = cells.column - preview.column;
                    marks[start..start + cells.columns].fill(mark);
                }
            }
            let style = Style {
                cursor: 'o',
                whitespace: false,
            };
            let (text, cells) = match screen_row(&screen, index, preview.columns) {
                Some(row) => draw_row(&display, &row, cursor, None, style),
                None => blank_row(preview.columns),
            };
            // The cursor's mark shows in the text, as the row of marks is the list's.
            text.chars()
                .zip(cells)
                .map(|(c, mark)| if mark == 'o' { 'o' } else { c })
                .collect()
        }
        Some(Preview::Withheld) if index == 0 => {
            let (text, _) = spans_row(&["may hold secrets"], preview.columns);
            text
        }
        // A kill's lines, as the float draws them (ADR 0091).
        Some(Preview::Text { entry }) => {
            let lines = editor.kill_preview(entry, preview.rows, preview.columns);
            let line = lines.get(index).map_or("", String::as_str);
            spans_row(&[line], preview.columns).0
        }
        _ => " ".repeat(preview.columns),
    };
    (format!("│{text}"), marks)
}
