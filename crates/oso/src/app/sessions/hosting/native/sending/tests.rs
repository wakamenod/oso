//! The messages being sent at a transcript's end, as its windows show them.
//!
//! Must not depend on: the app or a window's frame; the tests draw as it does.

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::frame::FrameSize;

use super::super::composer::new_docked_composer;
use super::super::markdown::tests::native;
use super::super::{Native, NativeState, draw, put_tail};
use crate::sessions::native::rows::Changes;
use crate::sessions::native::tail::Part;
use crate::sessions::native::wire::parse;

/// A running session shown in a window, its point at the end, as it follows.
fn session() -> (Editor, BufferId, Native) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 12,
        columns: 60,
    });
    let id = editor.add_native_session_buffer("*claude: s*");
    editor.set_wraps(id, true);
    editor.show(id);
    (editor, id, native(id))
}

/// Writes `text` to the session, as a send does.
fn send(editor: &mut Editor, native: &mut Native, text: &str) {
    let change = native.rows.sent(text);
    draw(editor, native, change);
}

fn feed(editor: &mut Editor, native: &mut Native, lines: &[&str]) {
    let mut changes = Changes::default();
    for line in lines {
        for change in native.rows.apply(parse(line.as_bytes())) {
            changes.add(change);
        }
    }
    draw(editor, native, changes);
}

fn text(editor: &Editor, id: BufferId) -> String {
    editor.buffer(id).unwrap().text().to_string()
}

const REPLAY: &str = r#"{"type":"user","isReplay":true,"message":{"content":"Fix it"}}"#;
const RESULT: &str = r#"{"type":"result","subtype":"success","duration_ms":900}"#;
const NOTE: &str = r#"{"type":"user","isReplay":true,"message":{"content":"Done"}}"#;

/// Drawn at once at the end, under what streams in meanwhile and over the queue and a block; its
/// row takes its place.
#[test]
fn a_message_shows_at_once_and_its_row_takes_its_place() {
    let (mut editor, id, mut native) = session();
    send(&mut editor, &mut native, "Fix it");
    assert_eq!(text(&editor, id), "…  Fix it\n");
    for (part, rows) in [(Part::Queue, "Queued (1)\n"), (Part::Body, "  ls\n")] {
        let runs = vec![(rows.len(), None)];
        put_tail(&mut editor, &mut native, part, rows.to_owned(), runs);
    }
    feed(&mut editor, &mut native, &[NOTE]);
    assert_eq!(text(&editor, id), "  Done\n\n…  Fix it\nQueued (1)\n  ls\n");
    // Point on the queue's row stays on it.
    let row = text(&editor, id).find("Queued").unwrap() + 3;
    editor.current_mut().0.selections.move_primary(row);
    feed(&mut editor, &mut native, &[REPLAY]);
    let shown = "  Done\n\n〉 Fix it\nQueued (1)\n  ls\n";
    assert_eq!(text(&editor, id), shown);
    let point = editor.window().selections.primary().cursor;
    assert_eq!(point, shown.find("Queued").unwrap() + 3);
}

#[test]
fn it_says_once_its_turn_ended() {
    let (mut editor, id, mut native) = session();
    send(&mut editor, &mut native, "Fix it");
    assert!(!draw(&mut editor, &mut native, []), "nothing changed");
    feed(&mut editor, &mut native, &[RESULT]);
    let shown = text(&editor, id);
    assert!(
        shown.contains("\n\n✗  Fix it\n   claude did not say"),
        "{shown}"
    );
}

/// A first message is no more than waiting while its process starts: nothing was written to it.
#[test]
fn nothing_is_being_sent_before_the_process_runs() {
    let (mut editor, id, mut native) = session();
    native.state = NativeState::Starting;
    send(&mut editor, &mut native, "Fix it");
    assert_eq!(text(&editor, id), "");
    native.state = NativeState::Running;
    assert!(super::sync(&mut editor, &mut native));
    assert_eq!(text(&editor, id), "…  Fix it\n");
}

/// The window at the end stays there with the same rows in view when the row takes the place of
/// what was being sent, and one the person reads further up does not move.
#[test]
fn a_window_follows_and_one_reading_stays() {
    for reading in [false, true] {
        let (mut editor, id, mut native) = session();
        for _ in 0..20 {
            feed(&mut editor, &mut native, &[NOTE]);
        }
        if reading {
            editor.current_mut().0.selections.move_primary(3);
            feed(&mut editor, &mut native, &[NOTE]);
        }
        let top = |editor: &Editor| editor.window().scroll();
        let cursor = |editor: &Editor| editor.window().selections.primary().cursor;
        let before = (top(&editor), cursor(&editor));
        send(&mut editor, &mut native, "Fix it");
        let sent = (top(&editor), cursor(&editor));
        feed(&mut editor, &mut native, &[REPLAY]);
        let end = text(&editor, id).len();
        if reading {
            assert_eq!((sent.1, cursor(&editor)), (before.1, 3));
            assert_eq!((sent.0, top(&editor)), (before.0, before.0));
        } else {
            assert_eq!(cursor(&editor), end);
            assert_eq!(top(&editor), sent.0, "no row moved");
        }
    }
}

/// At the process's end the text whose turn was under way is in a free composer again, and its note
/// says so; a composer that is not free is as it was.
#[test]
fn a_text_goes_back_to_a_free_composer_alone() {
    for free in [true, false] {
        let (mut editor, id, mut native) = session();
        let composer = new_docked_composer(&mut editor, id, "s");
        native.composing.buffer = Some(composer);
        send(&mut editor, &mut native, "Fix it");
        super::end(&mut editor, &mut native, free);
        let (held, said) = match free {
            true => ("Fix it", "and back in the composer: Fix it\n"),
            false => ("", "claude, which ended: Fix it\n"),
        };
        assert_eq!(text(&editor, composer), held);
        assert!(text(&editor, id).ends_with(said));
        assert_eq!(native.rows.sending().len(), 0);
    }
}

/// Its composer killed: the note says no more than that it ended.
#[test]
fn a_text_goes_back_to_no_killed_composer() {
    let (mut editor, id, mut native) = session();
    let composer = new_docked_composer(&mut editor, id, "s");
    native.composing.buffer = Some(composer);
    send(&mut editor, &mut native, "Fix it");
    editor.kill_buffer(composer);
    super::end(&mut editor, &mut native, true);
    assert!(text(&editor, id).ends_with("claude, which ended: Fix it\n"));
}

/// A transcript killed and not swept yet is left alone.
#[test]
fn a_killed_transcript_is_not_drawn() {
    let (mut editor, id, mut native) = session();
    send(&mut editor, &mut native, "Fix it");
    editor.kill_buffer(id);
    assert!(editor.buffer(id).is_none());
    feed(&mut editor, &mut native, &[REPLAY, RESULT]);
    let _ = native.rows.sent("Next");
    assert!(!super::sync(&mut editor, &mut native));
}
