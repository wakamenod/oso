//! A composer docked under its transcript: its long lines wrap, and its window is as tall as
//! they take, so that its whole text shows.
//!
//! Must not depend on: the app or the window.

use oso_core::buffer::{BufferId, Replacement};
use oso_core::editor::Editor;
use oso_core::frame::FrameSize;
use oso_core::window::{DockSlot, Screen};

use super::new_docked_composer;

/// An editor showing a transcript in a frame 40 columns wide, its composer selected.
fn composing() -> (Editor, BufferId, BufferId) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 40,
    });
    let transcript = editor.add_native_session_buffer("*claude: s*");
    editor.set_wraps(transcript, true);
    editor.show(transcript);
    let composer = new_docked_composer(&mut editor, transcript, "s");
    let window = editor.dock_window(transcript, DockSlot::Composer).unwrap();
    editor.select_window(window);
    (editor, transcript, composer)
}

/// Types `text` a character at a time, as keys or an input method's commits do.
fn typed(editor: &mut Editor, composer: BufferId, text: &str) {
    for c in text.chars() {
        let at = editor.buffer(composer).unwrap().text().len();
        let mut bytes = [0; 4];
        let typed = Replacement {
            range: at..at,
            text: c.encode_utf8(&mut bytes),
        };
        editor.refill_wraps();
        editor.apply_edits(composer, &[typed]).unwrap();
    }
}

/// The text the composer's window shows, its rows one after the other, and how many they are.
fn shown(editor: &Editor, composer: BufferId) -> (String, usize) {
    let (window, buffer) = editor.current();
    assert_eq!(window.buffer(), composer);
    let mut screen = Screen::default();
    editor.text_screen(&mut screen);
    let text = buffer.text().to_string();
    let rows = screen.rows();
    let shown = rows.iter().map(|row| &text[row.start..row.end]).collect();
    (shown, rows.len())
}

#[test]
fn a_long_japanese_message_shows_whole_as_it_is_typed_pasted_and_deleted() {
    let (mut editor, transcript, composer) = composing();
    assert!(editor.buffer(composer).unwrap().wraps());
    let long = "折り返しを試す日本語の長い文章です。".repeat(6);
    for end in (0..long.len()).filter(|&end| long.is_char_boundary(end)) {
        let have = editor.buffer(composer).unwrap().text().len();
        typed(&mut editor, composer, &long[have..end]);
        let (text, rows) = shown(&editor, composer);
        assert_eq!(text, long[..end], "at {end} bytes, {rows} rows");
    }
    let (_, rows) = shown(&editor, composer);
    assert!((2..=10).contains(&rows), "{rows} rows");
    let window = editor.dock_window(transcript, DockSlot::Composer).unwrap();
    assert_eq!(editor.regions(window).unwrap().text.rows, rows);
    // Emptied, it is a row again; pasted at once, as tall as before.
    let all = editor.buffer(composer).unwrap().text().len();
    let emptied = Replacement {
        range: 0..all,
        text: "",
    };
    editor.apply_edits(composer, &[emptied]).unwrap();
    assert_eq!(shown(&editor, composer), (String::new(), 1));
    let pasted = Replacement {
        range: 0..0,
        text: &long,
    };
    editor.apply_edits(composer, &[pasted]).unwrap();
    assert_eq!(shown(&editor, composer), (long, rows));
}
