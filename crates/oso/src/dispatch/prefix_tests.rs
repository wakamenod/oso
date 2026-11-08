//! Scenario tests of the prefix argument, with its keys, through the [`Dispatcher`]
//! (ADR 0037, ADR 0058).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every
//! OS with no window.

use super::motion_tests::{NOTHING, at, cursors, send, text};
use crate::commands::kept_messages;

#[test]
fn c_u_is_four_and_repeats_multiply() {
    let text16 = "x".repeat(40);
    let (mut d, mut editor) = at(&text16, &[0]);
    assert_eq!(send(&mut d, &mut editor, "C-u"), NOTHING);
    assert_eq!(cursors(&editor), [0], "it waits");
    send(&mut d, &mut editor, "C-f");
    assert_eq!(cursors(&editor), [4]);
    send(&mut d, &mut editor, "C-f");
    assert_eq!(cursors(&editor), [5], "used up");
    send(&mut d, &mut editor, "C-u C-u C-f");
    assert_eq!(cursors(&editor), [21]);
}

#[test]
fn digits_after_c_u_build_a_number() {
    let (mut d, mut editor) = at(&"x".repeat(200), &[0]);
    send(&mut d, &mut editor, "C-u 1 2 C-f");
    assert_eq!(cursors(&editor), [12]);
    send(&mut d, &mut editor, "C-u 0 C-f");
    assert_eq!(cursors(&editor), [12], "a count of 0");
    send(&mut d, &mut editor, "C-u 0 7 C-f");
    assert_eq!(cursors(&editor), [19]);
    send(&mut d, &mut editor, "C-u - 3 C-f");
    assert_eq!(cursors(&editor), [16]);
    send(&mut d, &mut editor, "C-u - C-b");
    assert_eq!(cursors(&editor), [17]);
    assert_eq!(text(&editor).len(), 200, "no digit was typed");
}

#[test]
fn meta_digits_begin_a_number_and_plain_digits_go_on() {
    let (mut d, mut editor) = at(&"x".repeat(200), &[0]);
    send(&mut d, &mut editor, "M-5 C-f");
    assert_eq!(cursors(&editor), [5]);
    send(&mut d, &mut editor, "M-1 0 C-f");
    assert_eq!(cursors(&editor), [15]);
    send(&mut d, &mut editor, "M-1 M-2 C-f");
    assert_eq!(cursors(&editor), [27]);
    send(&mut d, &mut editor, "M-- 2 C-f");
    assert_eq!(cursors(&editor), [25]);
    send(&mut d, &mut editor, "M-5 M-- C-f");
    assert_eq!(cursors(&editor), [20]);
}

#[test]
fn a_digit_with_no_prefix_is_text_and_a_prefix_repeats_it() {
    let (mut d, mut editor) = at("", &[0]);
    send(&mut d, &mut editor, "5 -");
    assert_eq!(text(&editor), "5-");
    send(&mut d, &mut editor, "C-u a");
    assert_eq!(text(&editor), "5-aaaa");
    send(&mut d, &mut editor, "C-u 2 b");
    assert_eq!(text(&editor), "5-aaaabb");
    send(&mut d, &mut editor, "C-u 3 C-u 7");
    assert_eq!(text(&editor), "5-aaaabb777", "C-u ended the number");
    send(&mut d, &mut editor, "C-u 2 RET");
    assert_eq!(text(&editor), "5-aaaabb777\n\n");
}

#[test]
fn a_minus_sign_after_digits_is_text_as_in_emacs() {
    let (mut d, mut editor) = at("", &[0]);
    send(&mut d, &mut editor, "C-u 3 -");
    assert_eq!(text(&editor), "---");
    send(&mut d, &mut editor, "M-2 -");
    assert_eq!(text(&editor), "-----");
    send(&mut d, &mut editor, "M-- 2 -");
    assert_eq!(
        kept_messages(&editor).len(),
        1,
        "a negative count to typing"
    );
    assert_eq!(text(&editor), "-----");
}

#[test]
fn a_further_c_u_after_a_closed_number_starts_over() {
    let (mut d, mut editor) = at("", &[0]);
    send(&mut d, &mut editor, "C-u 5 C-u C-u a");
    assert_eq!(text(&editor), "aaaa");
}

#[test]
fn a_negative_count_to_typing_signals_and_changes_nothing() {
    let (mut d, mut editor) = at("", &[0]);
    assert_eq!(send(&mut d, &mut editor, "M-- a"), NOTHING);
    assert_eq!(text(&editor), "");
    assert_eq!(kept_messages(&editor).len(), 1);
}

#[test]
fn a_count_goes_to_del_too() {
    let (mut d, mut editor) = at("abcdef", &[6]);
    send(&mut d, &mut editor, "C-u 2 DEL");
    assert_eq!(text(&editor), "abcd");
    send(&mut d, &mut editor, "C-u C-u DEL");
    assert_eq!(text(&editor), "abcd", "past the start: nothing");
    send(&mut d, &mut editor, "M-- DEL");
    assert_eq!(text(&editor), "abcd", "nothing after the cursor");
}

#[test]
fn c_g_cancels_the_prefix_at_any_point() {
    let (mut d, mut editor) = at("abcdefgh", &[0]);
    assert_eq!(send(&mut d, &mut editor, "C-u C-g"), NOTHING);
    send(&mut d, &mut editor, "C-f");
    assert_eq!(cursors(&editor), [1]);
    send(&mut d, &mut editor, "C-u 3 C-g C-f");
    assert_eq!(cursors(&editor), [2]);
    send(&mut d, &mut editor, "C-u C-x C-g C-f");
    assert_eq!(cursors(&editor), [3], "with a pending prefix key too");
    send(&mut d, &mut editor, "C-u C-x C-f");
    send(&mut d, &mut editor, "C-f");
    assert_eq!(cursors(&editor), [4], "an undefined sequence drops it");
}

#[test]
fn a_prefix_key_after_the_prefix_keeps_it_for_the_command() {
    // Only the digits' own layer changes what a digit means, and not after C-x.
    let (mut d, mut editor) = at("abc", &[0]);
    send(&mut d, &mut editor, "C-u 2 C-x 5");
    assert_eq!(cursors(&editor), [0], "C-x 5 is undefined, as without it");
    assert_eq!(text(&editor), "abc");
    assert!(d.pending().is_empty());
    let effects = send(&mut d, &mut editor, "C-u 2 C-x C-c");
    assert!(effects.exit, "a command that takes no count ignores it");
}

#[test]
fn a_huge_number_saturates_and_commands_limit_their_work() {
    let (mut d, mut editor) = at("abc", &[0]);
    send(
        &mut d,
        &mut editor,
        "C-u 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 C-f",
    );
    assert_eq!(cursors(&editor), [3], "past the edge, with no walk");
    send(
        &mut d,
        &mut editor,
        "M-- 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 9 C-f",
    );
    assert_eq!(cursors(&editor), [0]);
    send(
        &mut d,
        &mut editor,
        "C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-u C-f",
    );
    assert_eq!(cursors(&editor), [3]);
}

#[test]
fn a_huge_count_to_typing_is_refused_and_one_to_undo_ends_with_the_history() {
    let (mut d, mut editor) = at("", &[0]);
    assert_eq!(
        send(&mut d, &mut editor, "C-u 9 9 9 9 9 9 9 9 9 a"),
        NOTHING
    );
    assert_eq!(text(&editor), "");
    send(&mut d, &mut editor, "x");
    send(&mut d, &mut editor, "C-u 9 9 9 9 9 9 C-z");
    assert_eq!(text(&editor), "");
    assert_eq!(
        kept_messages(&editor).last().map(String::as_str),
        Some("No further undo information")
    );
}

#[test]
fn undo_stops_after_what_a_command_crosses() {
    let (mut d, mut editor) = at("", &[0]);
    for _ in 0..257 {
        send(&mut d, &mut editor, "RET");
    }
    send(&mut d, &mut editor, "C-u 9 9 9 C-z");
    assert_eq!(text(&editor), "\n", "all but one");
    assert_eq!(
        kept_messages(&editor).last().map(String::as_str),
        Some("Count too large")
    );
}

#[test]
fn a_prefix_from_the_keys_reaches_a_command_by_name() {
    let (mut d, mut editor) = at("abcdef", &[0]);
    send(&mut d, &mut editor, "C-u 3");
    let (_, result) = d.run_command(&mut editor, "forward-char", None);
    assert_eq!(result, Ok(()));
    assert_eq!(cursors(&editor), [3]);
    send(&mut d, &mut editor, "C-u 3");
    let (_, result) = d.run_command(&mut editor, "forward-char", Some(1));
    assert_eq!(result, Ok(()));
    assert_eq!(cursors(&editor), [4], "its own argument wins");
    send(&mut d, &mut editor, "C-f");
    assert_eq!(cursors(&editor), [5]);
}

#[test]
fn text_from_an_input_method_takes_the_prefix() {
    let (mut d, mut editor) = at("", &[0]);
    send(&mut d, &mut editor, "C-u 2");
    d.insert_text(&mut editor, "語句");
    assert_eq!(text(&editor), "語句語句");
    d.insert_text(&mut editor, "x");
    assert_eq!(text(&editor), "語句語句x");
}

/// A modified buffer, so that exiting is refused once.
fn modified() -> (super::Dispatcher, oso_core::editor::Editor) {
    let (mut d, mut editor) = at("", &[0]);
    send(&mut d, &mut editor, "a");
    (d, editor)
}

#[test]
fn a_prefix_does_not_answer_the_exit_question() {
    for before in ["C-u", "M-5", "M--", "C-u 3"] {
        let (mut d, mut editor) = modified();
        assert!(!send(&mut d, &mut editor, "C-x C-c").exit, "{before}");
        assert!(!send(&mut d, &mut editor, before).exit, "{before}");
        assert!(send(&mut d, &mut editor, "y").exit, "{before}");
    }
}

#[test]
fn a_prefix_given_to_an_exit_changes_nothing_about_the_question() {
    let (mut d, mut editor) = modified();
    assert!(!send(&mut d, &mut editor, "C-u C-x C-c").exit);
    assert!(send(&mut d, &mut editor, "y").exit);
}

#[test]
fn a_refused_exit_leaves_no_prefix_for_the_next_key() {
    let (mut d, mut editor) = modified();
    send(&mut d, &mut editor, "C-u C-x C-c n x");
    assert_eq!(text(&editor), "ax", "one x, not four");
}
