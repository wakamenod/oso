//! The rows a window shows of a body (ADR 0182), from a real window's screen.
//!
//! Must not depend on: the app; the tests give the window's size and its text.

use oso_core::editor::Editor;
use oso_core::frame::FrameSize;
use oso_core::window::Screen;

use super::*;

/// The rows of `body`, after a line of other text, in a window `columns` cells wide.
fn rows_in(body: &str, columns: usize, height: usize) -> Vec<Row> {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize { rows: 40, columns });
    let id = editor.add_native_session_buffer("*claude: s*");
    editor.show(id);
    editor.set_wraps(id, true);
    editor.replace_native_rows(id, 0..0, &format!("title\n{body}"), &[]);
    let (mut screen, mut out) = (Screen::default(), Vec::new());
    editor.screen_of(editor.window().id(), &mut screen);
    let buffer = editor.buffer(id).unwrap();
    rows::rows(
        &screen,
        &buffer.display(),
        height,
        &(6..6 + body.len()),
        &mut out,
    );
    out
}

#[test]
fn rows_cover_the_body_and_none_is_wider_than_its_window() {
    for body in [
        "$ a\n  b\n\n  c\n",
        &format!("$ {}\n", "word ".repeat(60)),
        &format!("$ {}\n+\n", "x".repeat(200)),
        &format!("$ {}\n", "あ".repeat(100)),
        &format!("$ {}\n", "\u{202e}\t\u{1b}".repeat(20)),
    ] {
        for columns in 20..60 {
            let rows = rows_in(body, columns, 40);
            let mut at = 0;
            for row in &rows {
                assert_eq!(row.start, at, "{columns}: {body:?}");
                assert!(row.cells <= columns, "{columns}: {row:?} {body:?}");
                at = row.end;
            }
            if rows.last().is_some_and(|row| row.at < 38) {
                assert_eq!(at, body.len(), "{columns}: {body:?}");
            }
            // One past the window is not among them.
            assert!(rows_in(body, columns, 2).iter().all(|row| row.at < 2));
        }
    }
    let rows = rows_in("$ ab\n\n", 40, 40);
    let expected = [
        Row {
            start: 0,
            end: 5,
            at: 1,
            cells: 4,
        },
        Row {
            start: 5,
            end: 6,
            at: 2,
            cells: 0,
        },
    ];
    assert_eq!(rows, expected);
}

#[test]
fn a_row_wider_than_its_window_says_so() {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 40,
    });
    let id = editor.add_native_session_buffer("*claude: s*");
    editor.show(id);
    let body = format!("$ {}\n", "x".repeat(100));
    editor.replace_native_rows(id, 0..0, &body, &[]);
    let (mut screen, mut out) = (Screen::default(), Vec::new());
    editor.screen_of(editor.window().id(), &mut screen);
    let buffer = editor.buffer(id).unwrap();
    rows::rows(&screen, &buffer.display(), 40, &(0..body.len()), &mut out);
    assert_eq!(out.iter().map(|row| row.cells).collect::<Vec<_>>(), [102]);
}
