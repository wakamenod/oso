//! Scenario tests of the user's keys, just above the global keymap, through the
//! [`Dispatcher`](super::Dispatcher) (ADR 0108).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every
//! OS with no window.

use oso_core::editor::{Editor, Status};
use oso_core::key::KeySequence;
use oso_core::keymap::Keymap;

use super::motion_tests::{at, cursors, send, text};
use crate::commands;

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(&mut status);
    status.echo
}

/// A user keymap binding each of `bind` and leaving each of `unbind` undefined.
fn user(bind: &[(&str, &str)], unbind: &[&str]) -> Keymap {
    let mut keymap = Keymap::from_table(&commands::registry(), bind).unwrap();
    for keys in unbind {
        let keys: KeySequence = keys.parse().unwrap();
        keymap.unbind(&keys.0).unwrap();
    }
    keymap
}

#[test]
fn a_user_key_runs_its_command_and_the_defaults_stay() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    d.set_user_keymap(user(
        &[("C-b", "forward-char"), ("C-c f", "forward-char")],
        &[],
    ));
    send(&mut d, &mut editor, "C-b");
    assert_eq!(cursors(&editor), [4]);
    send(&mut d, &mut editor, "C-c f C-f");
    assert_eq!(cursors(&editor), [6]);
    send(&mut d, &mut editor, "C-x h");
    assert_eq!(cursors(&editor), [0], "a default under C-x");
}

#[test]
fn an_undefined_user_key_says_so_and_does_nothing() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    d.set_user_keymap(user(&[], &["C-f", "C-x"]));
    send(&mut d, &mut editor, "C-f");
    assert_eq!(cursors(&editor), [3]);
    assert_eq!(echo(&editor), "C-f is undefined");
    send(&mut d, &mut editor, "C-x");
    assert_eq!(echo(&editor), "C-x is undefined");
    send(&mut d, &mut editor, "h");
    assert_eq!(
        text(&editor),
        "abchdef",
        "h is typed, and C-x h does not run"
    );
    d.set_user_keymap(Keymap::new());
    send(&mut d, &mut editor, "C-f");
    assert_eq!(cursors(&editor), [5], "an empty keymap takes them away");
}

#[test]
fn a_key_that_ends_a_search_runs_the_user_s_binding() {
    let (mut d, mut editor) = at("xx abc", &[0]);
    d.set_user_keymap(user(&[("C-b", "forward-char")], &[]));
    send(&mut d, &mut editor, "C-s a");
    assert_eq!(cursors(&editor), [4]);
    send(&mut d, &mut editor, "C-b");
    assert!(editor.isearch().is_none());
    assert_eq!(cursors(&editor), [5]);
}

#[test]
fn keys_given_during_a_prefix_wait_for_it_to_resolve() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    send(&mut d, &mut editor, "C-x");
    d.set_user_keymap(user(&[], &["C-x"]));
    send(&mut d, &mut editor, "h");
    assert_eq!(
        cursors(&editor),
        [0],
        "C-x h ran in the keymaps it started in"
    );
    send(&mut d, &mut editor, "C-x");
    assert_eq!(echo(&editor), "C-x is undefined");
}

#[test]
fn m_x_shows_the_user_s_keys_and_not_the_defaults_they_hide() {
    let (mut d, editor) = at("abc", &[0]);
    d.set_user_keymap(user(&[("C-c s", "save-buffer")], &["C-x C-s", "C-y"]));
    let items = d.command_candidates(&editor);
    let binding = |name: &str| {
        items
            .iter()
            .find(|item| item.text() == name)
            .map(|item| item.binding().to_owned())
    };
    assert_eq!(binding("save-buffer").as_deref(), Some("C-c s"));
    assert_eq!(binding("yank").as_deref(), Some(""), "listed, with no key");
    assert_eq!(binding("forward-char").as_deref(), Some("C-f"));
}

#[test]
fn a_question_never_reads_the_user_s_keys() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    let mut keymap = user(&[("y", "backward-char"), ("C-c f", "forward-char")], &[]);
    keymap
        .unbind(&"C-g".parse::<KeySequence>().unwrap().0)
        .unwrap();
    d.set_user_keymap(keymap);
    editor.ask("Go on?", "forward-char").unwrap();
    send(&mut d, &mut editor, "C-c f");
    assert!(editor.prompt().is_some(), "C-c is not a key of a question");
    send(&mut d, &mut editor, "y");
    assert!(editor.prompt().is_none());
    assert_eq!(
        cursors(&editor),
        [4],
        "y answered, as the question's own key"
    );
    editor.ask("Go on?", "forward-char").unwrap();
    send(&mut d, &mut editor, "C-g");
    assert!(editor.prompt().is_none(), "C-g still quits it");
    assert_eq!(cursors(&editor), [4]);
}

#[test]
fn a_search_under_way_reads_its_own_keys_before_the_user_s() {
    let (mut d, mut editor) = at("xx ab ab", &[0]);
    d.set_user_keymap(user(&[("C-r", "forward-char")], &["a"]));
    send(&mut d, &mut editor, "C-s a b");
    assert_eq!(cursors(&editor), [5], "a was searched for");
    send(&mut d, &mut editor, "C-r");
    assert!(
        editor.isearch().is_some(),
        "C-r searched backward, and ran no motion"
    );
}

#[test]
fn c_g_quits_a_prefix_of_the_user_s() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    let mut keymap = user(&[("C-c f", "forward-char")], &["C-x"]);
    keymap
        .bind(
            &"C-x k".parse::<KeySequence>().unwrap().0,
            commands::registry().lookup("forward-char").unwrap(),
        )
        .unwrap();
    d.set_user_keymap(keymap);
    for keys in ["C-c C-g", "C-x C-g"] {
        send(&mut d, &mut editor, keys);
        assert_eq!(echo(&editor), "Quit", "{keys}");
    }
    assert_eq!(cursors(&editor), [3]);
}

#[test]
fn m_x_leaves_out_what_a_user_command_cuts_short_and_what_is_not_active() {
    let (mut d, editor) = at("abc", &[0]);
    d.set_user_keymap(user(&[("C-x", "forward-char")], &[]));
    let items = d.command_candidates(&editor);
    let binding = |name: &str| {
        items
            .iter()
            .find(|item| item.text() == name)
            .map(|item| item.binding().to_owned())
    };
    assert!(binding("forward-char").is_some());
    assert_eq!(
        binding("save-buffer").as_deref(),
        Some(""),
        "C-x C-s is cut short"
    );
    for name in [
        "self-insert-command",
        "digit-argument",
        "answer-yes",
        "exit-minibuffer",
    ] {
        assert!(binding(name).is_none(), "{name}");
    }
}

#[cfg(feature = "dev")]
#[test]
fn a_typing_key_left_undefined_stays_off_the_screen_in_a_passphrase() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    d.set_user_keymap(user(&[], &["a"]));
    send(&mut d, &mut editor, "<f34> h a");
    assert_eq!(echo(&editor), "<char> is undefined");
    send(&mut d, &mut editor, "C-g a");
    assert_eq!(
        echo(&editor),
        "a is undefined",
        "beside nothing secret, as Emacs says"
    );
}

#[cfg(feature = "dev")]
#[test]
fn a_user_prefix_of_typing_keys_stays_off_the_screen_in_a_passphrase() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    d.set_user_keymap(user(
        &[("a b", "forward-char"), ("C-c a b", "forward-char")],
        &[],
    ));
    send(&mut d, &mut editor, "<f34> h a");
    assert_eq!(echo(&editor), "<char>-", "the keys in progress");
    send(&mut d, &mut editor, "x");
    assert_eq!(echo(&editor), "<char> <char> is undefined");
    send(&mut d, &mut editor, "C-c a");
    assert_eq!(echo(&editor), "C-c <char>-");
    send(&mut d, &mut editor, "C-g C-g a");
    assert_eq!(echo(&editor), "a-", "beside nothing secret, as Emacs says");
}

#[test]
fn the_log_masks_every_typing_key_and_the_screen_only_when_asked() {
    let keys: KeySequence = "C-x a <f5> Ж".parse().unwrap();
    assert_eq!(super::masked(&keys.0, true), "C-x <char> <f5> <char>");
    assert_eq!(super::masked(&keys.0, false), "C-x a <f5> Ж");
}

#[test]
fn c_g_is_never_the_user_s() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    let mut keymap = user(
        &[
            ("C-g", "forward-char"),
            ("C-c C-g", "forward-char"),
            ("C-c C-G", "forward-char"),
            ("C-x C-G", "forward-char"),
        ],
        &[],
    );
    keymap
        .bind(
            &"C-G x".parse::<KeySequence>().unwrap().0,
            commands::registry().lookup("forward-char").unwrap(),
        )
        .unwrap();
    d.set_user_keymap(keymap);
    for keys in ["C-g", "C-c C-g", "C-G", "C-c C-G", "C-x C-G"] {
        send(&mut d, &mut editor, keys);
        assert_eq!(echo(&editor), "Quit", "{keys}");
    }
    assert_eq!(cursors(&editor), [3]);
    d.set_user_keymap(user(&[], &["C-g"]));
    send(&mut d, &mut editor, "C-g");
    assert_eq!(echo(&editor), "Quit", "an undefined C-g too");
    // M-x lists what it lists without the user's C-g, and forward-char by its own key.
    let listed = |d: &super::Dispatcher| {
        let items = d.command_candidates(&editor);
        let find = |name: &str| {
            items
                .iter()
                .find(|item| item.text() == name)
                .map(|item| item.binding().to_owned())
        };
        (find("keyboard-quit"), find("forward-char"))
    };
    let undefined = listed(&d);
    d.set_user_keymap(user(
        &[("C-g", "forward-char"), ("C-c C-g", "forward-char")],
        &[],
    ));
    let bound = listed(&d);
    d.set_user_keymap(Keymap::new());
    assert_eq!(undefined, listed(&d));
    assert_eq!(bound, listed(&d));
}

#[test]
fn a_prefix_argument_s_digits_win_over_the_user_s_keys() {
    let (mut d, mut editor) = at(&"x".repeat(40), &[0]);
    d.set_user_keymap(user(&[("1", "backward-char"), ("-", "backward-char")], &[]));
    send(&mut d, &mut editor, "C-u 1 2 C-f");
    assert_eq!(cursors(&editor), [12]);
    send(&mut d, &mut editor, "C-u - 2 C-f");
    assert_eq!(cursors(&editor), [10]);
    send(&mut d, &mut editor, "1");
    assert_eq!(cursors(&editor), [9], "on its own, the user's key");
}
