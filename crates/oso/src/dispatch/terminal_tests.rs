//! Keys in a terminal through the dispatcher (ADR 0120, Q12): which reach the program as
//! requests for the host, which oso keeps, the `C-c` prefix, `C-q`, char mode, and that keys
//! sent to a program are not recorded into a keyboard macro.
//!
//! Must not depend on: `app`, winit or the terminal's thread; the host's part is the
//! requests the editor holds.

use oso_core::editor::{Editor, TerminalRequest};
use oso_core::key::{KeyPress, KeySequence};
use oso_core::window::ViewSize;

use super::Dispatcher;
use crate::terminal::route::Input;

fn keys(text: &str) -> Vec<KeyPress> {
    text.parse::<KeySequence>().unwrap().0
}

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, text: &str) {
    dispatcher.handle_keys(editor, keys(text));
}

fn in_terminal() -> (Dispatcher, Editor) {
    let mut editor = Editor::default();
    editor.set_view_size(ViewSize::new(10, 40));
    let terminal = editor.add_terminal_buffer("*terminal*");
    editor.show(terminal);
    (Dispatcher::new(), editor)
}

/// The keys the host was asked to send, one request's after another's.
fn sent(editor: &mut Editor) -> Vec<KeyPress> {
    editor
        .take_terminal_requests()
        .into_iter()
        .flat_map(|request| match request {
            TerminalRequest::Keys(keys) => keys,
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn typing_goes_to_the_program() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "l s SPC RET C-a M-b <up> C-g");
    assert_eq!(sent(&mut editor), keys("l s SPC RET C-a M-b <up> C-g"));
    assert!(editor.current().1.text().is_empty(), "nothing is inserted");
}

#[test]
fn c_x_stays_oso_s_and_c_c_c_c_sends_c_c() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "C-x 2");
    assert_eq!(editor.frame().windows().len(), 2);
    assert!(sent(&mut editor).is_empty());
    send(&mut dispatcher, &mut editor, "C-c C-c");
    assert_eq!(sent(&mut editor), keys("C-c"));
    // Another key after C-c sends both.
    send(&mut dispatcher, &mut editor, "C-c a");
    assert_eq!(sent(&mut editor), keys("C-c a"));
}

#[test]
fn c_q_sends_the_next_key_as_it_is() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "C-q");
    let id = editor.window().buffer();
    for request in editor.take_terminal_requests() {
        assert_eq!(request, TerminalRequest::QuoteNext);
        dispatcher.quote_next(id);
    }
    send(&mut dispatcher, &mut editor, "C-x");
    assert_eq!(sent(&mut editor), keys("C-x"));
    // Only the one key.
    send(&mut dispatcher, &mut editor, "C-x 2");
    assert_eq!(editor.frame().windows().len(), 2);
}

#[test]
fn char_mode_sends_everything_but_c_c_c_k() {
    let (mut dispatcher, mut editor) = in_terminal();
    let id = editor.window().buffer();
    send(&mut dispatcher, &mut editor, "C-c C-k");
    assert_eq!(
        editor.take_terminal_requests(),
        [TerminalRequest::ToggleCharMode]
    );
    dispatcher.toggle_char_mode(id);
    assert_eq!(dispatcher.terminal_input(id), Input::Char);
    send(&mut dispatcher, &mut editor, "C-x 2 M-x C-c C-c");
    assert_eq!(sent(&mut editor), keys("C-x 2 M-x C-c C-c"));
    assert_eq!(editor.frame().windows().len(), 1);
    send(&mut dispatcher, &mut editor, "C-c C-k");
    assert_eq!(dispatcher.terminal_input(id), Input::SemiChar);
    assert!(sent(&mut editor).is_empty());
}

#[test]
fn a_prompt_takes_its_keys() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "M-x");
    assert!(editor.prompt().is_some());
    send(&mut dispatcher, &mut editor, "a b C-g");
    assert!(editor.prompt().is_none());
    assert!(sent(&mut editor).is_empty());
}

#[test]
fn keys_sent_to_a_program_are_not_recorded_into_a_macro() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "C-x ( s e c r e t C-x )");
    assert_eq!(sent(&mut editor), keys("s e c r e t"));
    send(&mut dispatcher, &mut editor, "C-x e");
    assert!(sent(&mut editor).is_empty(), "the macro holds none of them");
}
