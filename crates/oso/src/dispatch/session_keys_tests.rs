//! The user's `[keys.session]` through the dispatcher (ADR 0132, step 2): above a session's
//! terminal's keys, `C-c` keys that run, keys left undefined, the line `C-c ?` says, and the
//! swap that waits for a pending prefix key.
//!
//! Must not depend on: `app`, winit or the terminal's thread.

use oso_core::editor::SessionRequest;
use oso_core::key::KeySequence;
use oso_core::keymap::Keymap;

use super::terminal_tests::{echo, in_session, in_terminal, keys, send, sent};
use crate::commands;

/// A user's session keymap binding each of `bind` and leaving each of `unbind` undefined.
fn user(bind: &[(&str, &str)], unbind: &[&str]) -> Keymap {
    let mut keymap = Keymap::from_table(&commands::registry(), bind).unwrap();
    for keys in unbind {
        let keys: KeySequence = keys.parse().unwrap();
        keymap.unbind(&keys.0).unwrap();
    }
    keymap
}

/// `C-c g` and `C-c x y` bound, `C-c n` left undefined.
fn mine() -> Keymap {
    user(
        &[
            ("C-c g", "session-next-done"),
            ("C-c x y", "session-next-waiting"),
        ],
        &["C-c n"],
    )
}

#[test]
fn a_session_s_user_keys_run_over_its_own_and_leave_keys_undefined() {
    let (mut dispatcher, mut editor) = in_session();
    assert_eq!(
        dispatcher.set_session_keys(mine(), None),
        Some(Keymap::new())
    );
    send(&mut dispatcher, &mut editor, "C-c g C-c x y C-c d");
    assert_eq!(
        editor.take_session_requests(),
        [
            SessionRequest::NextDone,
            SessionRequest::NextWaiting,
            SessionRequest::NextDone,
        ],
        "the defaults the user's keys leave stay"
    );
    send(&mut dispatcher, &mut editor, "C-c n");
    assert!(editor.take_session_requests().is_empty());
    assert_eq!(echo(&editor), "C-c n is undefined (C-c C-c sends C-c)");
    assert!(sent(&mut dispatcher, &mut editor).is_empty());
    // A prefix argument reaches them too.
    send(&mut dispatcher, &mut editor, "C-c C-u C-c g");
    assert_eq!(editor.take_session_requests(), [SessionRequest::NextDone]);
    assert!(!dispatcher.keys_in_progress());
    assert!(sent(&mut dispatcher, &mut editor).is_empty());
}

#[test]
fn a_plain_terminal_reads_no_session_keys() {
    let (mut dispatcher, mut editor) = in_terminal();
    dispatcher.set_session_keys(mine(), None);
    send(&mut dispatcher, &mut editor, "C-c g");
    assert!(editor.take_session_requests().is_empty());
    assert_eq!(sent(&mut dispatcher, &mut editor), keys("C-c g"));
}

#[test]
fn c_c_question_mark_says_the_line_given_with_the_keys() {
    let (mut dispatcher, mut editor) = in_session();
    dispatcher.set_session_keys(mine(), Some("C-c then:  g next-done".to_owned()));
    send(&mut dispatcher, &mut editor, "C-c ?");
    assert_eq!(echo(&editor), "C-c then:  g next-done");
    dispatcher.set_session_keys(Keymap::new(), None);
    send(&mut dispatcher, &mut editor, "C-c ?");
    assert!(
        echo(&editor).starts_with("C-c then:  a adopt"),
        "the default line"
    );
}

#[test]
fn session_keys_wait_for_a_pending_prefix_key() {
    let (mut dispatcher, mut editor) = in_session();
    send(&mut dispatcher, &mut editor, "C-c");
    assert_eq!(dispatcher.set_session_keys(mine(), None), None);
    // The key after `C-c` reads the keymaps `C-c` started in.
    send(&mut dispatcher, &mut editor, "g");
    assert_eq!(echo(&editor), "C-c g is undefined (C-c C-c sends C-c)");
    send(&mut dispatcher, &mut editor, "C-c g");
    assert_eq!(editor.take_session_requests(), [SessionRequest::NextDone]);
    assert_eq!(dispatcher.take_replaced_keymap(), Some(Keymap::new()));
    assert_eq!(dispatcher.take_replaced_keymap(), None);
}
