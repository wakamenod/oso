//! Keys in a terminal through the dispatcher (ADR 0120, Q12): which reach the program as
//! requests for the host, which oso keeps, the `C-c` prefix, `C-q`, char mode, and how keys
//! sent to a program are recorded into a keyboard macro, but never at a password prompt.
//!
//! Must not depend on: `app`, winit or the terminal's thread; the host's part is the
//! requests the editor holds.

use oso_core::editor::Editor;
use oso_core::key::{KeyPress, KeySequence};
use oso_core::window::ViewSize;

use super::{Dispatcher, TerminalSend};
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

/// The keys waiting to go to programs, one send's after another's.
fn sent(dispatcher: &mut Dispatcher, editor: &mut Editor) -> Vec<KeyPress> {
    dispatcher
        .take_terminal_sends(editor)
        .into_iter()
        .flat_map(|(_, send)| match send {
            TerminalSend::Keys(keys) => keys,
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn typing_goes_to_the_program() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "l s SPC RET C-a M-b <up> C-g");
    assert_eq!(
        sent(&mut dispatcher, &mut editor),
        keys("l s SPC RET C-a M-b <up> C-g")
    );
    assert!(editor.current().1.text().is_empty(), "nothing is inserted");
}

#[test]
fn c_x_stays_oso_s_and_c_c_c_c_sends_c_c() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "C-x 2");
    assert_eq!(editor.frame().windows().len(), 2);
    assert!(sent(&mut dispatcher, &mut editor).is_empty());
    send(&mut dispatcher, &mut editor, "C-c C-c");
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("C-c"));
    // Another key after C-c sends both.
    send(&mut dispatcher, &mut editor, "C-c a");
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("C-c a"));
}

#[test]
fn c_q_sends_the_next_key_as_it_is() {
    let (mut dispatcher, mut editor) = in_terminal();
    // In one batch, as the dev socket and a macro's replay send keys.
    send(&mut dispatcher, &mut editor, "C-q C-x");
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("C-x"));
    // Only the one key.
    send(&mut dispatcher, &mut editor, "C-x 2");
    assert_eq!(editor.frame().windows().len(), 2);
}

#[test]
fn char_mode_sends_everything_but_c_c_c_k() {
    let (mut dispatcher, mut editor) = in_terminal();
    let id = editor.window().buffer();
    send(
        &mut dispatcher,
        &mut editor,
        "C-c C-k C-x 2 M-x C-c C-c C-c a",
    );
    assert_eq!(dispatcher.terminal_input(id), Input::Char);
    // `C-c C-c` sends one `C-c`, and `C-c` with another key sends both.
    assert_eq!(
        sent(&mut dispatcher, &mut editor),
        keys("C-x 2 M-x C-c C-c a")
    );
    assert_eq!(editor.frame().windows().len(), 1);
    send(&mut dispatcher, &mut editor, "C-c C-k");
    assert_eq!(dispatcher.terminal_input(id), Input::SemiChar);
    assert!(sent(&mut dispatcher, &mut editor).is_empty());
}

#[test]
fn a_prompt_takes_its_keys() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "M-x");
    assert!(editor.prompt().is_some());
    send(&mut dispatcher, &mut editor, "a b C-g");
    assert!(editor.prompt().is_none());
    assert!(sent(&mut dispatcher, &mut editor).is_empty());
}

/// Runs the last macro to its end.
fn replay(dispatcher: &mut Dispatcher, editor: &mut Editor) {
    send(dispatcher, editor, "C-x e");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.macro_replaying() {
        dispatcher.replay(editor, until);
    }
}

#[test]
fn keys_sent_to_a_program_are_recorded_into_a_macro() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "C-x ( l s RET C-x )");
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("l s RET"));
    replay(&mut dispatcher, &mut editor);
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("l s RET"));
}

#[test]
fn text_sent_to_a_program_is_recorded_into_a_macro() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "C-x (");
    dispatcher.commit_text(&mut editor, "日本");
    send(&mut dispatcher, &mut editor, "C-x )");
    let texts = |dispatcher: &mut Dispatcher, editor: &mut Editor| -> Vec<String> {
        dispatcher
            .take_terminal_sends(editor)
            .into_iter()
            .filter_map(|(_, send)| match send {
                TerminalSend::Text(text) => Some(text),
                _ => None,
            })
            .collect()
    };
    assert_eq!(texts(&mut dispatcher, &mut editor), ["日本"]);
    replay(&mut dispatcher, &mut editor);
    assert_eq!(texts(&mut dispatcher, &mut editor), ["日本"]);
}

#[test]
fn a_c_c_sent_with_the_next_key_is_replayed_with_it() {
    let (mut dispatcher, mut editor) = in_terminal();
    send(&mut dispatcher, &mut editor, "C-x ( C-c a C-x 2 C-x )");
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("C-c a"));
    send(&mut dispatcher, &mut editor, "C-x 1");
    replay(&mut dispatcher, &mut editor);
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("C-c a"));
    assert_eq!(editor.frame().windows().len(), 2);
}

#[test]
fn typing_at_a_password_prompt_drops_the_recording() {
    let (mut dispatcher, mut editor) = in_terminal();
    let id = editor.current().0.buffer();
    editor.set_echo_off(id, true);
    send(&mut dispatcher, &mut editor, "C-x ( s e c r e t");
    assert!(!editor.macro_recording());
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("s e c r e t"));
    send(&mut dispatcher, &mut editor, "C-x )");
    replay(&mut dispatcher, &mut editor);
    assert!(
        sent(&mut dispatcher, &mut editor).is_empty(),
        "no macro holds it"
    );
}

#[test]
fn a_password_prompt_heard_of_after_the_keys_still_drops_the_recording() {
    let (mut dispatcher, mut editor) = in_terminal();
    let id = editor.current().0.buffer();
    send(&mut dispatcher, &mut editor, "C-x ( s e c r e t");
    assert!(editor.macro_recording());
    // The snapshot says so of a prompt that was open when the keys went.
    dispatcher.terminal_read_a_password(&mut editor, id);
    assert!(!editor.macro_recording());
}

#[test]
fn a_password_prompt_in_a_terminal_the_recording_never_typed_into_keeps_it() {
    let (mut dispatcher, mut editor) = in_terminal();
    let id = editor.current().0.buffer();
    send(&mut dispatcher, &mut editor, "C-x ( C-x 2");
    dispatcher.terminal_read_a_password(&mut editor, id);
    assert!(editor.macro_recording());
}

#[test]
fn a_replay_stops_before_it_types_at_a_password_prompt() {
    let (mut dispatcher, mut editor) = in_terminal();
    let id = editor.current().0.buffer();
    send(&mut dispatcher, &mut editor, "C-x ( a b C-x )");
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("a b"));
    editor.set_echo_off(id, true);
    replay(&mut dispatcher, &mut editor);
    assert!(sent(&mut dispatcher, &mut editor).is_empty());
    assert!(!editor.macro_replaying());
}

#[test]
fn keys_go_to_the_terminal_they_were_typed_in_whatever_is_selected_after() {
    let (mut dispatcher, mut editor) = in_terminal();
    let first = editor.window().buffer();
    send(&mut dispatcher, &mut editor, "C-x 2");
    let second = editor.add_terminal_buffer("*terminal*");
    send(&mut dispatcher, &mut editor, "C-x o");
    editor.show(second);
    send(&mut dispatcher, &mut editor, "C-x o a C-x o b C-c C-c");
    let sends = dispatcher.take_terminal_sends(&mut editor);
    assert_eq!(
        sends,
        [
            (first, TerminalSend::Keys(keys("a"))),
            (second, TerminalSend::Keys(keys("b"))),
            (second, TerminalSend::Keys(keys("C-c"))),
        ]
    );
}

#[test]
fn text_goes_like_a_key_and_waits_while_something_of_oso_s_is_open() {
    let (mut dispatcher, mut editor) = in_terminal();
    let id = editor.window().buffer();
    dispatcher.commit_text(&mut editor, "日本");
    send(&mut dispatcher, &mut editor, "C-x");
    // With `C-x` pending, the text is oso's: an undefined sequence, as in any buffer.
    dispatcher.commit_text(&mut editor, "語");
    assert_eq!(
        dispatcher.take_terminal_sends(&mut editor),
        [(id, TerminalSend::Text("日本".into()))]
    );
    assert!(
        editor.current().1.text().is_empty(),
        "nothing went into the buffer"
    );
}

#[test]
fn typed_text_carries_no_control_character() {
    let (mut dispatcher, mut editor) = in_terminal();
    let id = editor.window().buffer();
    dispatcher.insert_text(&mut editor, "a\u{1b}[201~b\tc\r\u{9b}d");
    assert_eq!(
        dispatcher.take_terminal_sends(&mut editor),
        [(id, TerminalSend::Text("a[201~b\tcd".into()))]
    );
}
