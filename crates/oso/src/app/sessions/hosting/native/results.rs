//! A native session's turns' results at the right edge of its window (ADR 0165, "The rows'
//! look"): spaces before each put it there, for the narrowest window that shows the session,
//! so that it wraps in none.
//!
//! Must not depend on: key presses or how a frame is drawn; `render` lays a row out for a
//! width, which this keeps.
//!
//! - A new result is drawn for the width kept. When the width changes, only those in view are
//!   drawn again, and the others as they come into view, so it costs what a window shows.

use std::ops::Range;

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;

use super::{Native, draw};
use crate::app::App;
use crate::sessions::native::rows::{Kind, RowChange};

impl App {
    /// After each event: each result in view laid out for another width is drawn again.
    pub(super) fn sync_native_results(&mut self) {
        let natives = &self.hosting.natives;
        let shows = |id| {
            (natives.map.values().chain(natives.aside.values())).any(|native| native.buffer == id)
        };
        if !(self.editor.windows()).any(|window| shows(window.buffer())) {
            return;
        }
        let mut shown = Vec::new();
        self.editor.shown_ranges(&mut shown);
        let mut redraw = false;
        let natives = &mut self.hosting.natives;
        for native in natives.map.values_mut().chain(natives.aside.values_mut()) {
            redraw |= lay(&mut self.editor, native, &shown);
        }
        if redraw {
            self.request_redraw();
        }
    }
}

/// Keeps the width of the narrowest window that shows `native`, and draws again its results in
/// view, as `shown` says, that were laid out for another. Whether any was.
fn lay(editor: &mut Editor, native: &mut Native, shown: &[(BufferId, Range<usize>)]) -> bool {
    let width = (editor.windows())
        .filter(|window| window.buffer() == native.buffer)
        .filter_map(|window| editor.regions(window.id()))
        // The last column is the wrap mark's.
        .map(|regions| regions.text.columns.saturating_sub(1))
        .min();
    let Some(width) = width else {
        return false;
    };
    native.width = width;
    let rows = native.rows.rows();
    let mut stale: Vec<usize> = Vec::new();
    for (_, range) in shown.iter().filter(|(id, _)| *id == native.buffer) {
        let Some(first) = native.placed.row_at(range.start) else {
            continue;
        };
        let last = (native.placed.row_at(range.end)).unwrap_or(rows.len());
        for (at, row) in rows.iter().enumerate().take(last + 1).skip(first) {
            if !matches!(row.kind, Kind::Result(_)) {
                continue;
            }
            if native.results.get(&at) != Some(&width) {
                stale.push(at);
            }
        }
    }
    stale.sort_unstable();
    stale.dedup();
    let any = !stale.is_empty();
    draw(editor, native, stale.into_iter().map(RowChange::Changed));
    any
}

#[cfg(test)]
mod tests {
    use oso_core::frame::FrameSize;

    use super::super::markdown::tests::native;
    use super::*;
    use crate::sessions::native::wire::parse;

    const RESULT: &[u8] = br#"{"type":"result","subtype":"success","duration_ms":1500}"#;

    fn sync(editor: &mut Editor, native: &mut Native) -> bool {
        let mut shown = Vec::new();
        editor.shown_ranges(&mut shown);
        lay(editor, native, &shown)
    }

    /// How wide each result is drawn, in columns.
    fn widths(editor: &Editor, native: &Native) -> Vec<usize> {
        let text = editor.buffer(native.buffer).unwrap().text().to_string();
        (text.lines().filter(|line| !line.is_empty()))
            .map(|line| line.chars().count())
            .collect()
    }

    #[test]
    fn results_in_view_stand_at_the_edge_of_the_narrowest_window() {
        let mut editor = Editor::default();
        let size = |rows, columns| FrameSize { rows, columns };
        editor.set_frame_size(size(12, 60));
        let id = editor.add_native_session_buffer("*claude: s*");
        editor.show(id);
        let mut native = native(id);
        assert!(!sync(&mut editor, &mut native), "nothing is drawn yet");
        for _ in 0..40 {
            let changes = native.rows.apply(parse(RESULT));
            draw(&mut editor, &mut native, changes);
        }
        let columns = |editor: &Editor| {
            let window = editor.window().id();
            editor.regions(window).unwrap().text.columns - 1
        };
        assert_eq!(widths(&editor, &native), vec![columns(&editor); 40]);
        assert!(!sync(&mut editor, &mut native), "laid out already");

        // Only what is in view is drawn again.
        editor.set_frame_size(size(12, 40));
        assert!(sync(&mut editor, &mut native));
        let narrow = columns(&editor);
        let drawn = widths(&editor, &native);
        let again = drawn.iter().filter(|width| **width == narrow).count();
        assert!((1..12).contains(&again), "{drawn:?}");
        assert!(!sync(&mut editor, &mut native), "once");
        // A new one takes the new width.
        let changes = native.rows.apply(parse(RESULT));
        draw(&mut editor, &mut native, changes);
        assert_eq!(widths(&editor, &native).last(), Some(&narrow));
    }
}
