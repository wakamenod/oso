//! What a native session's permission card draws behind its text (ADR 0180): the band behind
//! its title row, and a chip behind each key of its own. A chip is where the card's highlights
//! say a key is, which only `sessions::native::card` gives, and only to oso's own words: what
//! Claude's text holds is never looked at.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::buffer::Mode;
use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::syntax::Highlight;
use oso_core::window::{DockSlot, Screen, WindowId};
use oso_render::Face;

/// The face docked window `id` is drawn over, if it is one: a permission card's own, and the
/// float's for the others (ADR 0170).
pub(super) fn dock_face(editor: &Editor, id: WindowId) -> Option<Face> {
    match editor.window_by_id(id)?.docked()?.1 {
        DockSlot::Card => Some(Face::Card),
        DockSlot::Strip | DockSlot::Composer => Some(Face::Float),
    }
}

/// What a card fills behind its text, kept from frame to frame.
#[derive(Default)]
pub(super) struct CardFills {
    pub(super) title: Vec<CellRect>,
    pub(super) keys: Vec<CellRect>,
}

impl CardFills {
    /// Fills in what window `id`, as `screen` shows it, draws behind a card, if it shows one,
    /// which has no band of the current line's, `band`.
    pub(super) fn fill(
        &mut self,
        editor: &Editor,
        (id, screen): (WindowId, &Screen),
        band: &mut Vec<CellRect>,
    ) {
        self.title.clear();
        self.keys.clear();
        let Some(buffer) = (editor.window_by_id(id))
            .and_then(|window| editor.buffer(window.buffer()))
            .filter(|buffer| buffer.mode() == Mode::Card)
        else {
            return;
        };
        band.clear();
        let Some(regions) = editor.regions(id) else {
            return;
        };
        let text = regions.text;
        let (display, highlights) = (buffer.display(), buffer.highlights());
        for (index, row) in screen.rows().iter().take(text.rows).enumerate() {
            let on_row = CellRect {
                row: text.row + index,
                rows: 1,
                ..text
            };
            if row.line == 0 {
                self.title.push(on_row);
            }
            let Some(highlights) = highlights else {
                continue;
            };
            let (start, end) = (
                buffer.text().line_start(row.line),
                buffer.text().line_end(row.line),
            );
            for (bytes, highlight) in highlights.runs(start..end) {
                if highlight != Some(Highlight::CardKey) {
                    continue;
                }
                let column = |offset| display.offset_to_display_point(offset).column;
                let from = column(bytes.start).saturating_sub(row.column);
                let to = (column(bytes.end).saturating_sub(row.column)).min(text.columns);
                if from < to {
                    self.keys.push(CellRect {
                        column: text.column + from,
                        columns: to - from,
                        ..on_row
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use oso_core::frame::FrameSize;
    use oso_core::syntax::Highlights;

    use super::*;
    use crate::sessions::native::card::{Card, Question};
    use crate::sessions::native::render::Face as CardFace;

    #[test]
    fn a_cards_title_has_a_band_and_only_osos_keys_have_chips() {
        let mut editor = Editor::default();
        editor.set_frame_size(FrameSize {
            rows: 24,
            columns: 80,
        });
        let host = editor.add_native_session_buffer("*claude: s*");
        editor.show(host);
        let id = editor.add_internal_buffer("*card: s*", Mode::Card);
        // Claude's text says what a card's keys say.
        let card = Card::Call {
            title: "Bash wants to run".into(),
            lines: vec![("$  y  allow once   n  deny".into(), CardFace::Target)],
        };
        let shown = Question::new(card).shown(80, 12);
        editor.set_internal_text(id, &shown.text);
        let mut highlights = Highlights::new(shown.text.len());
        for (range, face) in &shown.faces {
            highlights.replace(range.clone(), [(range.len(), Some(face.highlight()))]);
        }
        let _ = editor.set_internal_highlights(id, highlights);
        editor.set_dock(host, DockSlot::Card, Some(id));
        let window = editor.dock_window(host, DockSlot::Card).unwrap();
        let text = editor.regions(window).unwrap().text;
        let mut screen = Screen::default();
        editor.screen_of(window, &mut screen);
        let (mut fills, mut band) = (CardFills::default(), vec![text]);
        fills.fill(&editor, (window, &screen), &mut band);
        assert!(band.is_empty(), "no current line's band");
        assert_eq!(fills.title.len(), 1);
        assert_eq!(
            (fills.title[0].row, fills.title[0].columns),
            (text.row, text.columns)
        );
        let keys: Vec<_> = (fills.keys.iter())
            .map(|r| (r.row - text.row, r.column - text.column, r.columns))
            .collect();
        // `╰─  y  allow once  n  deny …`: each key's cells with a space on each side.
        assert_eq!(keys[..2], [(2, 3, 3), (2, 18, 3)]);
        assert_eq!(keys.len(), 4, "y, n, N and RET: {keys:?}");
        assert!(keys.iter().all(|(row, ..)| *row == 2), "{keys:?}");
        // Another window draws none.
        let tree = editor.window().id();
        fills.fill(&editor, (tree, &screen), &mut Vec::new());
        assert!(fills.title.is_empty() && fills.keys.is_empty());
    }
}
