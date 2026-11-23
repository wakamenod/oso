//! The bands behind a native session's rows (ADR 0165, "The rows' look"): the person's
//! messages, and the lines a change adds and removes, each to the window's edges. A row has
//! its band by the highlight its line starts with, which only `sessions::native::render`
//! gives, and only there: what Claude's text holds is never looked at.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::syntax::Highlight;
use oso_core::window::{Screen, WindowId};

/// What a transcript fills behind its text, kept from frame to frame.
#[derive(Default)]
pub(super) struct TranscriptFills {
    pub(super) prompts: Vec<CellRect>,
    pub(super) added: Vec<CellRect>,
    pub(super) removed: Vec<CellRect>,
}

impl TranscriptFills {
    /// Fills in what window `id`, as `screen` shows it, draws behind a native session's rows,
    /// if it shows some: two seeks a row in view.
    pub(super) fn fill(&mut self, editor: &Editor, (id, screen): (WindowId, &Screen)) {
        self.prompts.clear();
        self.added.clear();
        self.removed.clear();
        let buffer = (editor.window_by_id(id))
            .and_then(|window| editor.buffer(window.buffer()))
            .filter(|buffer| buffer.is_native_session());
        let (Some(buffer), Some(regions)) = (buffer, editor.regions(id)) else {
            return;
        };
        let Some(highlights) = buffer.highlights() else {
            return;
        };
        let text = regions.text;
        for (index, row) in screen.rows().iter().take(text.rows).enumerate() {
            let rects = match highlights.at(buffer.text().line_start(row.line)) {
                Some(Highlight::SessionFront) => &mut self.prompts,
                Some(Highlight::DiffAdded) => &mut self.added,
                Some(Highlight::DiffRemoved) => &mut self.removed,
                _ => continue,
            };
            let at = text.row + index;
            match rects.last_mut() {
                Some(last) if last.row + last.rows == at => last.rows += 1,
                _ => rects.push(CellRect {
                    row: at,
                    rows: 1,
                    ..text
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use oso_core::frame::FrameSize;

    use super::*;

    #[test]
    fn a_band_is_where_oso_says_a_prompt_or_a_changed_line_is() {
        let mut editor = Editor::default();
        editor.set_frame_size(FrameSize {
            rows: 24,
            columns: 40,
        });
        let id = editor.add_native_session_buffer("*claude: s*");
        editor.show(id);
        let front = Some(Highlight::SessionFront);
        // A message of two lines and an empty one between, then a reply that says the same,
        // then a change.
        let rows = "〉 a\n\n   b\n\n  〉 a\n    + x\n    - y\n";
        let mut runs = vec![(4, front), (2, None), (1, front), (3, front), (3, None)];
        let reply = "  〉 a\n".len();
        runs.extend([(reply, None)]);
        runs.extend([(8, Some(Highlight::DiffAdded))]);
        runs.extend([(7, Some(Highlight::DiffRemoved))]);
        editor.replace_native_rows(id, 0..0, rows, &runs);
        let window = editor.window().id();
        let mut screen = Screen::default();
        editor.screen_of(window, &mut screen);
        let text = editor.regions(window).unwrap().text;
        let mut fills = TranscriptFills::default();
        fills.fill(&editor, (window, &screen));
        let shown = |rects: &[CellRect]| -> Vec<(usize, usize, usize)> {
            (rects.iter())
                .map(|r| (r.row - text.row, r.rows, r.columns))
                .collect()
        };
        assert_eq!(shown(&fills.prompts), [(0, 3, text.columns)]);
        assert_eq!(shown(&fills.added), [(5, 1, text.columns)]);
        assert_eq!(shown(&fills.removed), [(6, 1, text.columns)]);
        // Another buffer draws none, whatever its highlights.
        let other = editor.add_internal_buffer("*x*", oso_core::buffer::Mode::Fundamental);
        editor.set_internal_text(other, "a\n");
        editor.show(other);
        editor.screen_of(window, &mut screen);
        fills.fill(&editor, (window, &screen));
        assert!(fills.prompts.is_empty() && fills.added.is_empty() && fills.removed.is_empty());
    }
}
