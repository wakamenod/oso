//! The faces of a session's rows laid again: once a window shows it, in slices.
//!
//! Must not depend on: the syntax threads or a process.

use oso_core::syntax::Highlight;

use super::super::MAX_WAITING;
use super::super::faces::SLICE_ROWS;
use super::{REPLY, Setup, reported_on, setup, whole};

#[test]
fn a_session_not_shown_lays_its_faces_and_asks_once_shown() {
    let mut setup = setup();
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    let key = setup.opened()[0].0;
    setup.report(key, reported_on(REPLY, 5..9), true);
    let scratch = (setup.editor.buffers())
        .map(|(id, _)| id)
        .find(|id| *id != setup.buffer);
    let scratch = scratch.expect("scratch");
    setup.editor.show(scratch);
    setup.sync(false);
    setup.natives.replies.opener = Some(Box::new(setup.stand.clone()));
    let opened = setup.opened().len();

    // On while hidden: nothing is laid or asked for, and a reply whole since waits too.
    assert!(!setup.sync(true));
    setup.feed(&[whole("m2", "two")]);
    setup.sync(true);
    assert_eq!(setup.opened().len(), opened, "nothing asked while hidden");
    assert_eq!(setup.at(0), None);

    setup.editor.show(setup.buffer);
    assert!(setup.sync(true), "laid once shown");
    assert_eq!(setup.at(0), Some(Highlight::SessionQuiet));
    let texts: Vec<String> = setup.opened()[opened..]
        .iter()
        .map(|(.., text)| text.clone())
        .collect();
    assert_eq!(texts, ["two", REPLY], "then asked, the newest first");
}

#[test]
fn faces_are_laid_again_for_what_shows_at_once_and_the_rest_in_slices() {
    let mut setup = setup();
    let replies = 2 * SLICE_ROWS + 10;
    let lines: Vec<String> = (0..replies)
        .map(|n| whole(&format!("m{n}"), REPLY))
        .collect();
    setup.feed(&lines);
    setup.sync(true);
    setup.sync(false);
    // At the conversation's start: its first rows show.
    setup.editor.show(setup.buffer);
    (setup.editor.current_mut().0.selections).move_primary(0);
    setup.editor.follow_cursor();
    let width = setup.text().len() / replies;
    let heading = |setup: &Setup, row: usize| setup.at(row * width + 4);
    let marked = Some(Highlight::Heading);

    assert!(setup.sync(true), "a frame is due");
    assert_eq!(heading(&setup, 0), marked, "what shows");
    assert_eq!(heading(&setup, replies - 1), marked, "the newest");
    assert_eq!(heading(&setup, SLICE_ROWS), None, "waits");
    assert!(setup.natives.replies.waiting.is_empty(), "none asked yet");
    // A row drawn meanwhile has its faces, and is asked for with the others.
    setup.feed(&[whole("last", REPLY)]);
    assert!(setup.sync(true));
    assert_eq!(heading(&setup, SLICE_ROWS), marked);
    assert!(setup.natives.replies.waiting.is_empty());
    assert_eq!(heading(&setup, replies), marked);
    // Off and on again starts over.
    setup.sync(false);
    assert_eq!(heading(&setup, 0), None);
    for _ in 0..3 {
        assert!(setup.natives.replies.waiting.is_empty());
        assert!(setup.sync(true));
    }
    assert_eq!(setup.natives.replies.waiting.len(), MAX_WAITING);
    assert!((0..=replies).all(|row| heading(&setup, row) == marked));
    assert!(!setup.sync(true), "done");
}
