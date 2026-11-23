//! The `M-v` held while a dictation's text is due (ADR 0175), through the [`Dispatcher`]: what
//! it holds, what drops it, what lets it run, and the input held behind it, in order.
//!
//! Must not depend on: `app` or the clipboard; the clipboard's answer is given here, as the
//! host would.

use std::time::{Duration, Instant};

use oso_core::editor::Editor;

use super::motion_tests::{at, send, text};
use super::{Dispatcher, PasteGuard};

/// A guard open for a minute.
fn open() -> PasteGuard {
    PasteGuard::Until(Instant::now() + Duration::from_secs(60))
}

/// Forty lines, with the cursor on the last, so that `M-v` scrolls.
fn scrolled() -> (Dispatcher, Editor) {
    let lines: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let end = lines.len();
    let (d, mut editor) = at(&lines, &[end]);
    editor.follow_cursor();
    (d, editor)
}

#[test]
fn with_the_guard_off_m_v_scrolls_and_nothing_is_asked() {
    let (mut d, mut editor) = scrolled();
    let top = editor.top_row();
    send(&mut d, &mut editor, "M-v");
    assert!(editor.top_row() < top, "it scrolled");
    assert_eq!(d.take_paste_check(), None);
}

#[test]
fn typewhispers_command_v_with_its_marker_is_dropped() {
    let (mut d, mut editor) = scrolled();
    d.guard_pastes(open());
    let top = editor.top_row();
    send(&mut d, &mut editor, "M-v");
    assert_eq!(
        d.take_paste_check(),
        Some(false),
        "the clipboard is asked once"
    );
    assert_eq!(d.take_paste_check(), None);
    d.paste_checked(&mut editor, Some(true));
    assert_eq!(editor.top_row(), top, "nothing scrolled");
}

#[test]
fn the_persons_m_v_with_no_marker_or_no_answer_runs() {
    for answer in [Some(false), None] {
        let (mut d, mut editor) = scrolled();
        d.guard_pastes(open());
        let top = editor.top_row();
        send(&mut d, &mut editor, "M-v");
        assert_eq!(editor.top_row(), top, "held");
        assert!(d.take_paste_check().is_some());
        d.paste_checked(&mut editor, answer);
        assert!(editor.top_row() < top, "{answer:?}: it ran");
        assert_eq!(d.take_paste_check(), None, "and was not checked again");
    }
}

#[test]
fn keys_after_a_held_m_v_wait_behind_it_in_order() {
    let (mut d, mut editor) = at("", &[0]);
    d.guard_pastes(open());
    send(&mut d, &mut editor, "M-v a b");
    assert_eq!(text(&editor), "", "held behind it");
    assert!(d.take_paste_check().is_some());
    d.paste_checked(&mut editor, Some(true));
    assert_eq!(text(&editor), "ab");
}

#[test]
fn a_second_m_v_behind_the_first_is_checked_in_its_turn() {
    let (mut d, mut editor) = at("", &[0]);
    d.guard_pastes(open());
    send(&mut d, &mut editor, "M-v x M-v y");
    assert!(d.take_paste_check().is_some());
    d.paste_checked(&mut editor, Some(true));
    assert_eq!(text(&editor), "x", "the second waits for its own look");
    assert!(d.take_paste_check().is_some());
    d.paste_checked(&mut editor, Some(true));
    assert_eq!(text(&editor), "xy");
}

#[test]
fn only_m_v_alone_is_held_and_only_while_the_guard_is_open() {
    let (mut d, mut editor) = at("", &[0]);
    d.guard_pastes(open());
    send(&mut d, &mut editor, "v C-M-v");
    assert_eq!(d.take_paste_check(), None);
    assert_eq!(text(&editor), "v");
    d.guard_pastes(PasteGuard::Until(Instant::now() + Duration::from_secs(60)));
    send(&mut d, &mut editor, "M-v");
    assert!(d.take_paste_check().is_some(), "still open");
    d.paste_checked(&mut editor, Some(true));
    d.guard_pastes(PasteGuard::Until(Instant::now()));
    send(&mut d, &mut editor, "M-v");
    assert_eq!(d.take_paste_check(), None, "closed");
    d.guard_pastes(open());
    d.guard_pastes(PasteGuard::Off);
    send(&mut d, &mut editor, "M-v");
    assert_eq!(d.take_paste_check(), None);
}

#[test]
fn an_m_v_behind_a_yank_waits_in_the_yanks_queue_and_keeps_its_order() {
    let (mut d, mut editor) = at("", &[0]);
    d.guard_pastes(open());
    send(&mut d, &mut editor, "C-y a M-v b");
    assert!(d.take_clipboard_request().is_some(), "the yank reads first");
    assert_eq!(d.take_paste_check(), None, "M-v waits behind the yank");
    d.clipboard_answered(&mut editor, &oso_core::kill::FromClipboard::Nothing);
    assert_eq!(text(&editor), "a", "b waits behind the M-v");
    assert!(d.take_paste_check().is_some());
    d.paste_checked(&mut editor, Some(true));
    assert_eq!(text(&editor), "ab");
}

#[test]
fn a_keyboard_macros_m_v_is_never_held() {
    let (mut d, mut editor) = scrolled();
    send(&mut d, &mut editor, "C-x ( M-v x C-x )");
    d.guard_pastes(open());
    send(&mut d, &mut editor, "C-x e");
    let _ = d.replay(&mut editor, Instant::now() + Duration::from_secs(5));
    assert!(!editor.macro_replaying(), "the replay ran through");
    assert_eq!(d.take_paste_check(), None);
    assert_eq!(text(&editor).matches('x').count(), 2);
}
