//! The echo area: its line, and in isearch what an input method is composing after the search
//! string, underlined as in a buffer, with what the line says after it (ADR 0158).
//!
//! Must not depend on: key presses or commands; it reads what `Status` says.

use oso_core::display_map::text_clusters;
use oso_core::editor::{Editor, Status};
use oso_core::frame::CellRect;
use oso_render::{
    Bands, Face, GridCursor, GridCursorShape, GridView, Layer, Pane, PaneContent, PaneId, Rules,
    Span,
};

/// The echo area's pane, apart from every window's and float's.
pub(super) const ECHO: PaneId = PaneId(2);
/// A composition's on it, and that of what the line says after one.
const COMPOSED: PaneId = PaneId(1 << 24);
const AFTER: PaneId = PaneId((1 << 24) + 1);
/// The grid the composition is drawn as, which no buffer's id is.
const GRID: u64 = u64::MAX;

/// Where in the line something is composed now.
fn composed(status: &Status) -> Option<std::ops::Range<usize>> {
    status.composed.clone().filter(|range| !range.is_empty())
}

/// What the echo area says, its warnings first, and what comes after a composition last.
pub(super) fn spans(status: &Status) -> [Span<'_>; 3] {
    let end = status.echo.len();
    let (before, after) = composed(status).map_or((end, end), |range| (range.start, range.end));
    let span = |text, face| Span { text, face };
    [
        span(status.echo_warnings(), Face::Warning),
        span(
            status.echo.get(..before).unwrap_or(&status.echo),
            Face::Text,
        ),
        span(status.echo.get(after..).unwrap_or_default(), Face::Text),
    ]
}

/// Their lines: what is before a composition, then what is after it.
pub(super) fn lines<'a>(spans: &'a [Span<'a>; 3]) -> [&'a [Span<'a>]; 2] {
    let (before, after) = spans.split_at(2);
    [before, after]
}

/// The echo area's panes. They have no background of their own, so that nothing marks the
/// pixels left over below it (ADR 0057).
pub(super) fn panes<'a>(
    editor: &'a Editor,
    status: &Status,
    lines: &'a [&'a [Span<'a>]; 2],
    list: &mut Vec<Pane<'a>>,
) {
    let echo = editor.frame().echo;
    let pane = |id, region, content| Pane {
        id,
        region,
        layer: Layer::TILED,
        background: None,
        panel: None,
        bands: Bands::NONE,
        rules: Rules::NONE,
        content,
    };
    let preedit = editor.preedit().filter(|_| composed(status).is_some());
    let Some(preedit) = preedit else {
        list.push(pane(ECHO, echo, PaneContent::Lines(&lines[..1])));
        return;
    };
    let before: usize = lines[0].iter().map(|span| cells(span.text)).sum();
    // Its three parts side by side, in their cells, each cut to the row.
    let [start, end] =
        [before, before.saturating_add(preedit.cells())].map(|column| column.min(echo.columns));
    let part = |start: usize, end: usize| CellRect {
        column: echo.column + start,
        columns: end - start,
        ..echo
    };
    if start > 0 {
        list.push(pane(ECHO, part(0, start), PaneContent::Lines(&lines[..1])));
    }
    if start < end {
        // The caret, only where it is before the end: the buffer keeps the cursor.
        let cursor = preedit.caret().map(|_| GridCursor {
            row: 0,
            column: 0,
            shape: GridCursorShape::Block,
            hollow: true,
        });
        let view = GridView {
            id: GRID,
            rows: &[],
            cursor,
            preedit: Some(preedit),
            preedit_at: (0, 0),
        };
        list.push(pane(COMPOSED, part(start, end), PaneContent::Grid(view)));
    }
    if end < echo.columns {
        list.push(pane(
            AFTER,
            part(end, echo.columns),
            PaneContent::Lines(&lines[1..]),
        ));
    }
}

/// The cells `text` takes.
fn cells(text: &str) -> usize {
    text_clusters(text, 0).map(|cluster| cluster.cells).sum()
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Buffer;
    use oso_core::display_map::Preedit;
    use oso_core::editor::Editor;
    use oso_core::frame::FrameSize;

    use super::*;
    use crate::app::draw::{Cells, statuses_into, with_panes};

    /// What the echo row shows, pane by pane: its columns and its text.
    fn row(editor: &Editor) -> Vec<(usize, usize, String)> {
        let mut statuses = Vec::new();
        statuses_into(editor, &mut statuses);
        with_panes(editor, &statuses, &mut Cells::default(), |panes| {
            panes
                .iter()
                .filter(|pane| [ECHO, COMPOSED, AFTER].contains(&pane.id))
                .map(|pane| {
                    let shown = match &pane.content {
                        PaneContent::Lines(lines) => {
                            lines[0].iter().map(|span| span.text).collect()
                        }
                        PaneContent::Grid(view) => view.preedit.unwrap().text().to_owned(),
                        _ => panic!("lines or a composition"),
                    };
                    (pane.region.column, pane.region.columns, shown)
                })
                .collect()
        })
    }

    #[test]
    fn a_search_shows_its_composition_between_its_string_and_its_count() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", "日"));
        editor.set_frame_size(FrameSize {
            rows: 5,
            columns: 40,
        });
        editor.isearch_begin(false, false);
        editor.isearch_add("日");
        let whole = row(&editor);
        assert_eq!(whole.len(), 1);
        assert_eq!((whole[0].0, whole[0].1), (0, 40));
        editor.set_preedit(Some(Preedit::new("ほん".into(), None)));
        let parts = row(&editor);
        assert_eq!(parts[0], (0, 12, "I-search: 日".to_owned()));
        assert_eq!(parts[1], (12, 4, "ほん".to_owned()));
        assert_eq!((parts[2].0, parts[2].1), (16, 24));
        // Wider than the row: it alone.
        editor.set_preedit(Some(Preedit::new("か".repeat(30), None)));
        let parts = row(&editor);
        assert_eq!(parts.len(), 1);
        assert_eq!((parts[0].0, parts[0].1), (0, 40));
        // In a buffer, no search on, it is not the echo area's.
        editor.isearch_exit();
        assert_eq!(row(&editor).len(), 1);
    }
}
