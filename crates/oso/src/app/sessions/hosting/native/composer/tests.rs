//! A composer docked under its transcript: its long lines wrap, and its window is as tall as
//! they take, so that its whole text shows.
//!
//! Must not depend on: the app or the window.

use oso_core::buffer::{BufferId, Replacement};
use oso_core::editor::Editor;
use oso_core::frame::FrameSize;
use oso_core::window::{DockSlot, Screen};

use super::composed::put_at_end;
use super::{Composing, HIDDEN, Run, Turn, new_docked_composer};

impl Composing {
    /// A message queued and taken to be edited.
    pub(in crate::app::sessions::hosting::native) fn edits_a_queued(&mut self) {
        let turn = Turn {
            busy: true,
            run: Run::Running,
            leaving: false,
        };
        let _ = self.queue.send("queued".to_owned(), turn);
        let id = self.queue.waiting().next().map(|queued| queued.id);
        assert!(id.and_then(|id| self.queue.edit(id)).is_some());
    }
}

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

/// Whether the composer's window shows the end of its text, and how many columns it is
/// scrolled by.
fn shows_end(editor: &Editor, composer: BufferId) -> (bool, usize) {
    let (window, buffer) = editor.current();
    assert_eq!(window.buffer(), composer);
    let mut screen = Screen::default();
    editor.text_screen(&mut screen);
    let end = buffer.text().len();
    let last = screen.rows().last().is_some_and(|row| row.end == end);
    (last, window.scroll().left)
}

#[test]
fn what_is_put_at_the_end_is_in_view_with_point_after_it() {
    for (rows, wraps) in [(30, true), (12, true), (30, false), (12, false)] {
        let (mut editor, transcript, composer) = composing();
        editor.set_frame_size(FrameSize { rows, columns: 40 });
        editor.set_wraps(composer, wraps);
        typed(&mut editor, composer, &"line\n".repeat(14));
        let start = editor.current_mut().0;
        start.selections.move_primary(0);
        editor.follow_cursor();
        assert!(!shows_end(&editor, composer).0, "{rows} rows, {wraps}");
        // From the transcript, wider than the window.
        let host = editor
            .windows()
            .find(|w| w.buffer() == transcript)
            .unwrap()
            .id();
        editor.select_window(host);
        let text = "a suggestion ".repeat(5);
        put_at_end(&mut editor, (transcript, composer), &text).unwrap();
        let (end, left) = shows_end(&editor, composer);
        assert!(end, "{rows} rows, {wraps}");
        assert_eq!(left > 0, !wraps, "{rows} rows");
        let (window, buffer) = editor.current();
        assert_eq!(window.selections.len(), 1);
        assert_eq!(window.selections.primary().cursor, buffer.text().len());
    }
}

#[test]
fn nothing_is_put_into_a_hidden_composer_or_beneath_a_prompt() {
    let (mut editor, transcript, composer) = composing();
    let put = |editor: &mut Editor| put_at_end(editor, (transcript, composer), "x");
    editor.ask("Exit?", "keyboard-quit").unwrap();
    assert!(put(&mut editor).is_err());
    editor.quit();
    assert!(editor.prompt().is_none());
    editor.set_frame_size(FrameSize {
        rows: 3,
        columns: 40,
    });
    assert_eq!(put(&mut editor), Err(HIDDEN.to_owned()));
    assert!(editor.buffer(composer).unwrap().text().is_empty());
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 40,
    });
    assert_eq!(put(&mut editor), Ok(()));
}

#[test]
fn what_is_put_after_a_short_draft_shows_whole_with_it() {
    for columns in [40, 20, 14] {
        let (mut editor, transcript, composer) = composing();
        editor.set_frame_size(FrameSize { rows: 30, columns });
        typed(&mut editor, composer, "see this");
        let text = " run the tests and fix what fails";
        put_at_end(&mut editor, (transcript, composer), text).unwrap();
        let (seen, _) = shown(&editor, composer);
        assert_eq!(seen, format!("see this{text}"), "{columns} columns");
    }
}
