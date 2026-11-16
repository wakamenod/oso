//! Tests of a server's description at point (ADR 0138, Hover, step 10): where its float goes,
//! what it refuses, what closes it, its size, its scroll and its buffer.
//!
//! Must not depend on: threads or the file system.

use super::*;
use crate::editor::edit_tests::edit;
use crate::frame::FrameSize;
use crate::selection::SelectionSet;

fn area() -> CellRect {
    CellRect {
        row: 0,
        column: 4,
        rows: 30,
        columns: 100,
    }
}

#[test]
fn a_float_at_point_goes_below_its_row_or_above_with_more_room_and_never_over_it() {
    let below = frame::at_point(area(), (5, 10), (20, 40)).unwrap();
    assert_eq!((below.outer.row, below.outer.rows), (6, 20));
    assert_eq!((below.outer.column, below.outer.columns), (10, 42));
    assert_eq!((below.inner.column, below.inner.columns), (11, 40));
    // Near the bottom, above, as tall as the room there.
    let above = frame::at_point(area(), (25, 10), (20, 40)).unwrap();
    assert_eq!((above.outer.row, above.outer.rows), (5, 20));
    assert!(
        above.outer.row + above.outer.rows <= 25,
        "never over the row"
    );
    // Few rows fit either way: the side with more, cut.
    let cut = frame::at_point(area(), (14, 10), (40, 40)).unwrap();
    assert_eq!((cut.outer.row, cut.outer.rows), (15, 15));
    // Moved left to fit, and cut to the area's columns.
    let right = frame::at_point(area(), (5, 95), (3, 40)).unwrap();
    assert_eq!(right.outer.column + right.outer.columns, 104);
    let wide = frame::at_point(area(), (5, 10), (3, 500)).unwrap();
    assert_eq!((wide.outer.column, wide.outer.columns), (4, 100));
    // Outside the area, or with no room, nothing.
    assert!(frame::at_point(area(), (30, 10), (3, 4)).is_none());
    let one = CellRect { rows: 1, ..area() };
    assert!(frame::at_point(one, (0, 10), (3, 4)).is_none());
}

/// An editor with a frame of 40 by 120 showing a buffer of `text`, and where an ask there is.
fn asked(text: &str) -> (Editor, AskAt) {
    let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", text));
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 120,
    });
    let at = editor.ask_at();
    (editor, at)
}

#[test]
fn a_description_shows_in_its_own_buffer_only_while_nothing_moved() {
    let (mut editor, at) = asked("fn main() {}\n");
    assert!(editor.show_hover(at, "```rust\nfn main()\n```\nThe *entry*.", true));
    let shown = *editor.hover().unwrap();
    let buffer = editor.buffer(shown.buffer()).unwrap();
    assert!(buffer.is_internal() && buffer.withheld_from_agents());
    assert_eq!(buffer.mode(), Mode::Markdown);
    assert!(buffer.text().to_string().starts_with("```rust"));
    assert!(editor.buffer_named(DESCRIBED).is_none(), "no list shows it");
    assert!(editor.hover_layout().is_some());
    // A move of the cursor puts it away, and an edit would too.
    editor.windows[0].selections = SelectionSet::new(3);
    assert!(editor.hover().is_none() && editor.hover_layout().is_none());
    // An ask that something moved since shows nothing; nor does an empty text.
    let (mut editor, at) = asked("fn main() {}\n");
    let id = editor.window().buffer();
    editor.apply_edits(id, &[edit(0..0, "x")]).unwrap();
    assert!(!editor.show_hover(at, "text", false));
    let (mut editor, at) = asked("fn main() {}\n");
    assert!(!editor.show_hover(at, " \n ", false));
}

#[test]
fn its_float_fits_its_text_at_most_80_by_20_and_scrolls_by_pages() {
    let (mut editor, at) = asked("x\n");
    assert!(editor.show_hover(at, "short\nlonger line", false));
    let regions = editor.hover_layout().unwrap();
    assert_eq!((regions.inner.rows, regions.inner.columns), (2, 11));
    let long: String = (0..60)
        .map(|n| format!("line {n} {}\n", "y".repeat(100)))
        .collect();
    let (mut editor, at) = asked("x\n");
    assert!(editor.show_hover(at, &long, false));
    let regions = editor.hover_layout().unwrap();
    assert_eq!((regions.inner.rows, regions.inner.columns), (20, 80));
    assert!(!editor.scroll_hover(-1), "at the top already");
    assert_eq!(editor.hover_page(), 18);
    assert!(editor.scroll_hover(18));
    assert_eq!(editor.hover().unwrap().top(), 18);
    let mut screen = Screen::default();
    editor.hover_screen(&mut screen);
    assert_eq!(screen.rows().len(), 20);
    while editor.scroll_hover(18) {}
    assert!(editor.scroll_hover(-1));
    editor.close_hover();
    assert!(editor.hover().is_none());
}
