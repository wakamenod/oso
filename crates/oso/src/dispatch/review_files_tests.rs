//! A review's files pane and filter through the [`Dispatcher`] (ADR 0122, step 6b): `s` in the
//! review selects the pane, whose keymap is its own, `q` closes it, and `/` opens holding the
//! filter, `M-p` giving back an empty prompt and `RET` keeping what it holds.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use std::path::PathBuf;

use oso_core::buffer::{Buffer, Mode};
use oso_core::editor::{Editor, ReviewId};
use oso_core::kill::{Fingerprint, FromClipboard};

use super::Dispatcher;
use super::motion_tests::send;
use crate::commands::review::tests::{in_project, with_review};

#[test]
fn s_selects_the_pane_whose_keys_are_its_own_and_q_closes_it() {
    let (mut editor, project) = in_project(true);
    let review = with_review(&mut editor, project);
    let mut d = Dispatcher::new();
    send(&mut d, &mut editor, "s");
    assert_eq!(editor.current().1.mode(), Mode::ReviewFiles);
    assert_eq!(editor.review_files(review), Some(editor.window().buffer()));
    send(&mut d, &mut editor, "q");
    assert!(editor.review_files(review).is_none());
    assert_eq!(Some(editor.window().buffer()), editor.review_buffer(review));
}

#[test]
fn slash_opens_holding_the_filter_and_ret_keeps_it() {
    let (mut editor, project) = in_project(true);
    let review = with_review(&mut editor, project);
    let mut d = Dispatcher::new();
    send(&mut d, &mut editor, "/ s r c RET");
    assert_eq!(editor.review_filter(review).unwrap().text(), "src");
    send(&mut d, &mut editor, "/");
    assert_eq!(editor.current().1.text().to_string(), "src", "prefilled");
    send(&mut d, &mut editor, "SPC a RET");
    assert_eq!(editor.review_filter(review).unwrap().text(), "src a");
    // `M-p` gives back what was typed: nothing, which shows every file again.
    send(&mut d, &mut editor, "/ M-p RET");
    assert!(editor.prompt().is_none());
    assert!(editor.review_filter(review).is_none());
}

/// A dispatcher and an editor showing a review, with a kill of a buffer that may hold secrets
/// newest in the ring.
fn with_a_secret_kill() -> (Dispatcher, Editor, ReviewId) {
    let (mut editor, project) = in_project(true);
    let path = PathBuf::from("/home/me/src/oso/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.judge_secret(Some(&[path]));
    secret.finish_empty();
    let secret = editor.add_buffer(secret);
    editor.show(secret);
    let mut d = Dispatcher::new();
    d.insert_text(&mut editor, "TOKEN=abc");
    send(&mut d, &mut editor, "C-a C-k");
    let write = editor.take_clipboard_write().unwrap();
    editor.note_clipboard_written(write.id, Fingerprint { len: 9, hash: 42 });
    let review = with_review(&mut editor, project);
    (d, editor, review)
}

/// `C-y` of the kill the clipboard holds, this instance's own.
fn yank(d: &mut Dispatcher, editor: &mut Editor) {
    send(d, editor, "C-y");
    assert!(d.take_clipboard_request().is_some());
    d.clipboard_answered(editor, &FromClipboard::Own);
}

#[test]
fn a_filter_answered_from_a_prompt_that_may_hold_secrets_is_not_kept() {
    let (mut d, mut editor, review) = with_a_secret_kill();
    send(&mut d, &mut editor, "/");
    yank(&mut d, &mut editor);
    assert!(editor.current().1.is_secret());
    send(&mut d, &mut editor, "RET");
    assert!(editor.review_filter(review).is_none());
    assert!(editor.prompt().is_none());
}

#[test]
fn ret_and_spc_move_no_place_out_of_or_into_what_may_hold_secrets() {
    let (mut d, mut editor, review) = with_a_secret_kill();
    // The review turned secret by an isearch: SPC leaves its cursor alone.
    send(&mut d, &mut editor, "C-s C-y C-g C-g");
    assert!(editor.current().1.is_secret());
    let window = editor.window().id();
    let before = editor.window().selections.primary().cursor;
    send(&mut d, &mut editor, "s n n SPC");
    let after = editor
        .window_by_id(window)
        .unwrap()
        .selections
        .primary()
        .cursor;
    assert_eq!(after, before);

    // The pane turned secret: RET goes nowhere.
    let (mut d, mut editor, review_2) = with_a_secret_kill();
    send(&mut d, &mut editor, "s");
    assert_eq!(
        editor.review_files(review_2),
        Some(editor.window().buffer())
    );
    send(&mut d, &mut editor, "C-s C-y C-g C-g");
    assert!(editor.current().1.is_secret());
    send(&mut d, &mut editor, "RET");
    assert_eq!(
        editor.review_files(review_2),
        Some(editor.window().buffer()),
        "stays"
    );
    assert!(editor.review_filter(review).is_none());
}
