//! A review comment's draft and the review buffer's comment keys through the [`Dispatcher`]
//! (ADR 0122): `RET` breaks a line, `C-c C-c` keeps the draft, `C-g` puts it in the kill
//! ring alone and off the clipboard, and a draft too long or whose target is gone stays open.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::Replacement;
use oso_core::editor::{DraftFor, Editor, ReviewId, ReviewRequest, Status};
use oso_core::review::{Anchor, Author, MAX_COMMENT_BYTES, Side};

use super::Dispatcher;
use super::motion_tests::send;
use crate::commands::review::tests::{
    Comments, in_project, lines, lines_of, put_at, with_comments,
};

/// A dispatcher and an editor showing a review with comments, its cursor on `line`.
fn at(line: usize) -> (Dispatcher, Editor, ReviewId, Comments) {
    let (mut editor, project) = in_project(true);
    let (review, comments) = with_comments(&mut editor, project);
    put_at(&mut editor, &[line]);
    (Dispatcher::new(), editor, review, comments)
}

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

fn newest_kill(editor: &Editor) -> String {
    editor
        .kill_ring()
        .get(0)
        .map(|entry| entry.joined())
        .unwrap_or_default()
}

#[test]
fn ret_breaks_a_line_and_c_c_c_c_keeps_the_comment() {
    let (mut d, mut editor, review, _) = at(lines::OLD);
    send(&mut d, &mut editor, "c");
    assert!(editor.draft().is_some());
    send(&mut d, &mut editor, "h i RET t h e r e");
    assert!(editor.draft().is_some(), "RET does not keep it");
    assert_eq!(editor.current().1.text().to_string(), "hi\nthere");
    assert_eq!(editor.prompt_layout().unwrap().input.rows, 2);
    // No history to walk.
    send(&mut d, &mut editor, "M-p");
    assert_eq!(editor.current().1.text().to_string(), "hi\nthere");
    send(&mut d, &mut editor, "C-c C-c");
    assert!(editor.prompt().is_none());
    let model = editor.review(review).unwrap();
    let kept = model.comments().last().unwrap();
    assert_eq!(kept.author(), Author::Person);
    assert_eq!(kept.text(), "hi\nthere");
    assert_eq!(
        kept.anchor(),
        Anchor::Line {
            side: Side::Old,
            number: 1
        }
    );
    assert_eq!(
        editor.take_review_requests(),
        [ReviewRequest::Redraw {
            review,
            path: b"a".as_slice().into()
        }]
    );
    // The review buffer's keys again.
    assert_eq!(lines_of(&editor), [lines::OLD]);
}

#[test]
fn c_g_puts_the_draft_in_the_kill_ring() {
    let (mut d, mut editor, review, _) = at(lines::HUNK);
    let before = editor.review(review).unwrap().comments().count();
    send(&mut d, &mut editor, "c d r a f t C-g");
    assert!(editor.prompt().is_none());
    assert_eq!(newest_kill(&editor), "draft");
    assert!(editor.take_clipboard_write().is_none(), "ring only");
    assert_eq!(echo(&editor), "Draft in the kill ring: C-y");
    assert_eq!(editor.review(review).unwrap().comments().count(), before);
}

#[test]
fn a_draft_too_long_stays_open() {
    let (mut d, mut editor, review, _) = at(lines::OLD);
    send(&mut d, &mut editor, "c");
    let buffer = editor.prompt().unwrap().buffer();
    let long = "x".repeat(MAX_COMMENT_BYTES + 1);
    let edit = Replacement {
        range: 0..0,
        text: &long,
    };
    editor.apply_edits(buffer, &[edit]).unwrap();
    let before = editor.review(review).unwrap().comments().count();
    send(&mut d, &mut editor, "C-c C-c");
    assert!(editor.draft().is_some());
    assert_eq!(
        editor.prompt().and_then(|prompt| prompt.message()),
        Some("Too long: a comment holds 64 KiB at most")
    );
    assert_eq!(editor.review(review).unwrap().comments().count(), before);
}

#[test]
fn a_draft_whose_target_is_gone_stays_open_until_c_g_keeps_it() {
    let (mut d, mut editor, _, _) = at(lines::B_LINE);
    send(&mut d, &mut editor, "c g o n e C-c C-c");
    assert!(editor.draft().is_some());
    assert_eq!(
        editor.prompt().and_then(|prompt| prompt.message()),
        Some("Comment target gone: C-g keeps the text in the kill ring")
    );
    send(&mut d, &mut editor, "C-g");
    assert!(editor.prompt().is_none());
    assert_eq!(newest_kill(&editor), "gone");
    assert!(editor.take_clipboard_write().is_none());
}

#[test]
fn c_k_in_a_draft_then_c_g_keeps_the_draft_as_an_entry_of_its_own_off_the_clipboard() {
    let (mut d, mut editor, _, _) = at(lines::HUNK);
    send(&mut d, &mut editor, "c a b RET c d C-a C-k");
    assert_eq!(editor.take_clipboard_write().unwrap().text(), "cd");
    send(&mut d, &mut editor, "C-g");
    assert!(editor.prompt().is_none());
    let ring: Vec<String> = editor.kill_ring().iter().map(|e| e.joined()).collect();
    assert_eq!(ring, ["ab\n", "cd"]);
    assert!(editor.take_clipboard_write().is_none(), "the draft's text");
    assert_eq!(echo(&editor), "Draft in the kill ring: C-y");
}

#[test]
fn the_comment_keys_of_a_review_buffer() {
    let (mut d, mut editor, review, comments) = at(lines::TITLE);
    send(&mut d, &mut editor, "}");
    assert_eq!(lines_of(&editor), [lines::MINE]);
    send(&mut d, &mut editor, "C-u 2 }");
    assert_eq!(lines_of(&editor), [lines::AGENTS]);
    send(&mut d, &mut editor, "{");
    assert_eq!(lines_of(&editor), [lines::MY_REPLY]);
    send(&mut d, &mut editor, "e");
    assert_eq!(
        editor.draft().map(|(_, draft)| draft.clone()),
        Some(DraftFor::Edit {
            comment: comments.my_reply
        })
    );
    send(&mut d, &mut editor, "C-g C-g");
    assert!(editor.prompt().is_none());
    send(&mut d, &mut editor, "r");
    let model = editor.review(review).unwrap();
    assert!(model.comment(comments.mine).unwrap().is_resolved());
    send(&mut d, &mut editor, "a");
    assert!(editor.agents_folded(review));
    send(&mut d, &mut editor, "k");
    assert!(
        editor
            .review(review)
            .unwrap()
            .comment(comments.my_reply)
            .is_none()
    );
}
