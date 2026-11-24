//! The head of a block taller than its window, and the transcript's end after it, in an editor's
//! windows.
//!
//! Must not depend on: the app; the tests lay out an editor's windows.

use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::frame::{FrameSize, Way};
use oso_core::window::WindowId;
use serde_json::json;

use super::super::{put_title, runs};
use super::{first, free, in_block, show, to_end};
use crate::app::sessions::hosting::native::markdown::tests::native;
use crate::app::sessions::hosting::native::{Native, draw, put_tail};
use crate::sessions::native::request::Block;
use crate::sessions::native::tail::Part;
use crate::sessions::native::wire::parse;

struct Setup {
    editor: Editor,
    native: Native,
    transcript: BufferId,
    window: WindowId,
}

fn note(setup: &mut Setup, text: &str) {
    for change in setup.native.rows.apply(parse(reply(text).as_bytes())) {
        draw(&mut setup.editor, &mut setup.native, [change]);
    }
}

fn reply(text: &str) -> String {
    json!({"type": "user", "isReplay": true, "message": {"content": text}}).to_string()
}

/// A transcript of `rows` messages, its window at its end.
fn setup(rows: usize) -> Setup {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 20,
        columns: 60,
    });
    let transcript = editor.add_native_session_buffer("*claude: s*");
    editor.set_wraps(transcript, true);
    editor.show(transcript);
    let native = native(transcript);
    let window = editor.window().id();
    let mut setup = Setup {
        editor,
        native,
        transcript,
        window,
    };
    for row in 0..rows {
        note(&mut setup, &format!("message {row}"));
    }
    end(&mut setup, window);
    setup
}

/// Puts `window` at the transcript's end.
fn end(setup: &mut Setup, window: WindowId) {
    to_end(&mut setup.editor, setup.transcript, &[window]);
}

fn ask(setup: &mut Setup, lines: usize) {
    let command: Vec<String> = (0..lines).map(|n| format!("line {n}")).collect();
    let input = json!({"command": command.join("\n")});
    let block = Block::call("Bash", Some(&input), Instant::now());
    setup.editor.refill_wraps();
    put_title(&mut setup.editor, &mut setup.native, &block, ((1, 1), 59));
    let body = block.body();
    put_tail(
        &mut setup.editor,
        &mut setup.native,
        Part::Body,
        body.text.clone(),
        runs(body),
    );
}

/// Takes the block away, as `close_request` does.
fn answered(setup: &mut Setup) {
    let heads = in_block(&setup.editor, &setup.native);
    for part in [Part::Title, Part::Body] {
        put_tail(
            &mut setup.editor,
            &mut setup.native,
            part,
            String::new(),
            Vec::new(),
        );
    }
    to_end(&mut setup.editor, setup.transcript, &heads);
}

/// What `window` shows, a row a line.
fn shown(setup: &Setup, window: WindowId) -> String {
    let window = setup.editor.window_by_id(window).unwrap();
    let text = setup.editor.buffer(setup.transcript).unwrap().text();
    let first = text.offset_to_point(window.scroll().start).row;
    let rows: Vec<String> = (text.to_string().lines())
        .skip(first)
        .take(window.size().rows)
        .map(str::to_owned)
        .collect();
    rows.join("\n")
}

fn cursor(setup: &Setup, window: WindowId) -> usize {
    let window = setup.editor.window_by_id(window).unwrap();
    window.selections.primary().cursor
}

fn len(setup: &Setup) -> usize {
    setup.editor.buffer(setup.transcript).unwrap().text().len()
}

#[test]
fn a_window_that_follows_shows_the_head_of_a_block_taller_than_it() {
    let mut s = setup(12);
    ask(&mut s, 60);
    assert!(shown(&s, s.window).contains("line 59"), "it follows");
    assert!(show(&mut s.editor, &s.native, s.window));
    let top = shown(&s, s.window);
    assert!(top.starts_with("⚠ Bash asks to run"), "{top}");
    assert_eq!(cursor(&s, s.window), first(&s.native));
    // It follows no more: what the stream draws above moves neither.
    note(&mut s, "a row of a subagent");
    assert_eq!(shown(&s, s.window), top);
    assert_eq!(cursor(&s, s.window), first(&s.native));
}

#[test]
fn a_block_that_fits_and_a_window_that_does_not_follow_stay() {
    let mut s = setup(12);
    ask(&mut s, 5);
    assert!(!show(&mut s.editor, &s.native, s.window), "all of it shows");
    assert_eq!(cursor(&s, s.window), len(&s));

    answered(&mut s);
    ask(&mut s, 5);
    let before = s.editor.window_by_id(s.window).unwrap().scroll();
    assert!(in_block(&s.editor, &s.native).is_empty());
    answered(&mut s);
    let window = s.editor.window_by_id(s.window).unwrap();
    assert_eq!(window.scroll(), before, "nor as it goes");

    // The person reads in the block.
    let mut s = setup(12);
    ask(&mut s, 60);
    let mut at = s.editor.window_by_id(s.window).unwrap().selections.clone();
    at.move_primary(len(&s) - 9);
    let top = s.editor.window_by_id(s.window).unwrap().scroll();
    s.editor.restore_positions_in(s.window, at, top);
    assert!(!show(&mut s.editor, &s.native, s.window));

    // The person reads earlier rows.
    let mut s = setup(12);
    let mut window = s.editor.window_by_id(s.window).unwrap().selections.clone();
    window.move_primary(0);
    let top = oso_core::window::Scroll::default();
    s.editor.restore_positions_in(s.window, window, top);
    let before = shown(&s, s.window);
    ask(&mut s, 60);
    assert!(!show(&mut s.editor, &s.native, s.window));
    assert_eq!((shown(&s, s.window), cursor(&s, s.window)), (before, 0));
}

#[test]
fn a_transcript_in_two_windows_shows_the_head_in_the_one_that_follows() {
    let mut s = setup(12);
    let other = s.editor.split_window(Way::Down, false).unwrap();
    end(&mut s, other);
    let top = oso_core::window::Scroll::default();
    let mut selections = s.editor.window_by_id(s.window).unwrap().selections.clone();
    selections.move_primary(0);
    s.editor.restore_positions_in(s.window, selections, top);
    ask(&mut s, 60);
    assert!(!show(&mut s.editor, &s.native, s.window));
    assert!(show(&mut s.editor, &s.native, other));
    assert_eq!(
        (cursor(&s, s.window), cursor(&s, other)),
        (0, first(&s.native))
    );
}

#[test]
fn answered_the_window_is_at_the_end_again() {
    let mut s = setup(12);
    let before = shown(&s, s.window);
    ask(&mut s, 60);
    show(&mut s.editor, &s.native, s.window);
    answered(&mut s);
    assert_eq!(cursor(&s, s.window), len(&s));
    assert_eq!(shown(&s, s.window), before, "its last row at the bottom");
    note(&mut s, "and it follows");
    assert!(shown(&s, s.window).contains("and it follows"));

    // Read on to the block's end, point and view in the block.
    ask(&mut s, 60);
    show(&mut s.editor, &s.native, s.window);
    s.editor.scroll_view(s.window, 40);
    answered(&mut s);
    assert_eq!(cursor(&s, s.window), len(&s));
    assert!(shown(&s, s.window).contains("and it follows"));
}

#[test]
fn answered_a_window_the_person_moved_stays() {
    let mut s = setup(12);
    ask(&mut s, 60);
    show(&mut s.editor, &s.native, s.window);
    s.editor.scroll_view(s.window, -30);
    let before = (shown(&s, s.window), cursor(&s, s.window));
    assert!(before.1 < first(&s.native));
    answered(&mut s);
    assert_eq!((shown(&s, s.window), cursor(&s, s.window)), before);
}

/// A transcript with nothing above the block.
#[test]
fn the_head_of_a_transcripts_only_text() {
    let mut s = setup(0);
    ask(&mut s, 60);
    assert!(show(&mut s.editor, &s.native, s.window));
    answered(&mut s);
    assert_eq!(cursor(&s, s.window), len(&s));
}

/// A region and a search are the person's: their window is not taken to the head meanwhile.
#[test]
fn a_window_with_a_region_or_a_search_is_not_taken_to_the_head() {
    use oso_core::selection::MarkActive;
    let mut s = setup(12);
    assert!(free(&s.editor, s.window));
    let mark = |s: &mut Setup, active| {
        let (window, _) = s.editor.current_mut();
        window.selections.set_mark_active(active);
    };
    mark(&mut s, MarkActive::Active);
    assert!(!free(&s.editor, s.window));
    mark(&mut s, MarkActive::Inactive);
    let other = s.editor.split_window(Way::Down, false).unwrap();
    let _ = s.editor.isearch_begin(true, false);
    assert!(!free(&s.editor, s.window) && free(&s.editor, other));
}
