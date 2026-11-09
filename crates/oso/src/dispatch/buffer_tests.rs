//! Scenario tests of `C-x b` and `C-x k` run by keys (ADR 0088): the order, a new buffer, the
//! typed text, and killing a modified buffer.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use oso_core::buffer::Buffer;
use oso_core::complete::Candidate;
use oso_core::editor::{Editor, Status};
use oso_core::window::ViewSize;

use super::Dispatcher;
use super::motion_tests::send;

/// An editor on `b.txt`, which it went to from `a.txt`.
fn on_b() -> (Dispatcher, Editor) {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", "aaa\n"));
    editor.set_view_size(ViewSize::new(24, 80));
    let b = editor.add_buffer(Buffer::from_text("b.txt", "bbb\n"));
    editor.show(b);
    (Dispatcher::new(), editor)
}

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(&mut status);
    status.echo
}

fn shown(editor: &Editor) -> &str {
    editor.current().1.name()
}

fn selected(editor: &Editor) -> Option<&str> {
    let candidates = editor.prompt()?.candidates()?;
    candidates.selected_candidate().map(Candidate::text)
}

fn rows(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    candidates
        .shown(18)
        .map(|(_, candidate)| candidate.text().to_owned())
        .collect()
}

#[test]
fn c_x_b_ret_goes_back_to_the_buffer_before() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "C-x b");
    assert_eq!(editor.prompt().unwrap().label(), "Switch to buffer:");
    assert_eq!(rows(&editor), ["a.txt", "*Messages*", "b.txt"]);
    send(&mut d, &mut editor, "RET");
    assert!(editor.prompt().is_none());
    assert_eq!(shown(&editor), "a.txt");
    send(&mut d, &mut editor, "C-x b RET");
    assert_eq!(shown(&editor), "b.txt", "and back again");
}

#[test]
fn a_name_typed_that_matches_nothing_makes_a_buffer() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "C-x b z z z RET");
    assert_eq!(shown(&editor), "zzz");
    assert!(editor.current().1.path().is_none());
}

#[test]
fn m_ret_makes_a_buffer_named_as_typed_while_another_matches() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "C-x b a");
    assert_eq!(selected(&editor), Some("a.txt"));
    send(&mut d, &mut editor, "M-RET");
    assert_eq!(shown(&editor), "a");
}

#[test]
fn c_p_from_the_first_selects_what_is_typed() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "C-x b a");
    send(&mut d, &mut editor, "C-p");
    assert_eq!(selected(&editor), None);
    assert!(editor.prompt().unwrap().candidates().unwrap().on_input());
    send(&mut d, &mut editor, "C-n");
    assert_eq!(selected(&editor), Some("a.txt"), "back to the first");
    send(&mut d, &mut editor, "C-p RET");
    assert_eq!(shown(&editor), "a");
}

#[test]
fn c_x_k_kills_the_current_buffer_first_and_shows_the_one_before() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "C-x k");
    assert_eq!(rows(&editor), ["b.txt", "a.txt", "*Messages*"]);
    send(&mut d, &mut editor, "RET");
    assert_eq!(shown(&editor), "a.txt");
    assert!(editor.buffer_named("b.txt").is_none());
}

#[test]
fn c_x_k_takes_only_a_listed_buffer() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "C-x k z z z RET");
    assert_eq!(editor.prompt().unwrap().message(), Some("No match"));
}

#[test]
fn a_modified_buffer_asks_and_y_kills_it() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "x");
    assert!(editor.current().1.is_modified());
    send(&mut d, &mut editor, "C-x k RET");
    let question = editor.prompt().unwrap().label().to_owned();
    assert!(question.starts_with("b.txt is modified"), "{question}");
    send(&mut d, &mut editor, "n");
    assert_eq!(shown(&editor), "b.txt", "n keeps it");
    send(&mut d, &mut editor, "C-x k RET y");
    assert_eq!(shown(&editor), "a.txt");
    assert!(editor.buffer_named("b.txt").is_none());
}

#[test]
fn messages_cannot_be_killed() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "C-x k");
    send(&mut d, &mut editor, "C-n C-n RET");
    assert!(editor.buffer_named("*Messages*").is_some());
    assert_eq!(echo(&editor), "*Messages* is kept");
}

#[test]
fn the_buffer_keys_refuse_inside_a_prompt() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "M-g g C-x b");
    let prompt = editor.prompt().unwrap();
    assert_eq!(prompt.label(), "Go to line: ");
    assert_eq!(prompt.message(), Some("Not in the minibuffer"));
}

#[test]
fn c_dot_lists_the_marks_and_ret_goes_to_one() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "C-.");
    assert!(editor.prompt().is_none(), "no mark yet");
    assert_eq!(echo(&editor), "No mark set in this buffer");
    send(&mut d, &mut editor, "C-SPC C-SPC C-e");
    send(&mut d, &mut editor, "C-.");
    assert_eq!(editor.prompt().unwrap().label(), "Go to mark:");
    assert_eq!(rows(&editor), ["1:0: bbb"]);
    send(&mut d, &mut editor, "RET");
    assert_eq!(editor.window().selections.primary().cursor, 0);
    assert_eq!(editor.window().selections.primary().mark, Some(3));
}

/// [`on_b`], with a buffer that may hold secrets shown, which it went to from `b.txt`.
fn on_secret() -> (Dispatcher, Editor) {
    let (d, mut editor) = on_b();
    let path = std::path::PathBuf::from("/tmp/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.finish_empty();
    secret.judge_secret(Some(&[path]));
    let secret = editor.add_buffer(secret);
    editor.show(secret);
    (d, editor)
}

#[test]
fn c_dot_refuses_inside_a_prompt() {
    let (mut d, mut editor) = on_b();
    send(&mut d, &mut editor, "C-SPC C-SPC M-g g 1 C-SPC C-a C-.");
    let prompt = editor.prompt().unwrap();
    assert_eq!(
        prompt.label(),
        "Go to line: ",
        "no list of the prompt's text"
    );
    assert_eq!(prompt.message(), Some("Not in the minibuffer"));
}

#[test]
fn c_dot_never_previews_a_buffer_that_may_hold_secrets_and_forgets_its_list_on_c_g() {
    let (mut d, mut editor) = on_secret();
    send(&mut d, &mut editor, "C-SPC C-SPC C-.");
    assert!(editor.prompt().is_some());
    assert_eq!(editor.preview(), Some(oso_core::editor::Preview::Withheld));
    send(&mut d, &mut editor, "C-g");
    assert_eq!(editor.preview(), None);
}

#[test]
fn beside_a_secret_m_ret_on_nothing_typed_still_needs_a_choice() {
    let (mut d, mut editor) = on_secret();
    send(&mut d, &mut editor, "M-x M-RET");
    let prompt = editor.prompt().unwrap();
    assert_eq!(prompt.message(), Some("Type a name, or move to one"));
}

#[test]
fn beside_a_secret_c_x_b_ret_goes_back_as_anywhere() {
    let (mut d, mut editor) = on_secret();
    send(&mut d, &mut editor, "C-x b RET");
    assert!(editor.prompt().is_none());
    assert_eq!(shown(&editor), "b.txt");
}
