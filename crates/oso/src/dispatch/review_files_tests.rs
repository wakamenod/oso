//! A review's files pane and filter through the [`Dispatcher`] (ADR 0122, step 6b): `s` in the
//! review selects the pane, whose keymap is its own, `q` closes it, and `/` opens holding the
//! filter, `M-p` giving back an empty prompt and `RET` keeping what it holds.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::Mode;

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
