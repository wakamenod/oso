//! The tests of `[keys.session]`: the keys of a session's terminal, `C-c` sequences over its
//! default keys, and what `C-c ?` says with them (ADR 0132, step 2).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::fmt::Write;

use oso_core::keymap::Keymap;

use super::SessionKeys;
use super::keys::MAX_ENTRIES;
use super::tests::{Fixture, keys, messages};
use super::{Loaded, bindings};

/// The session keys of a file that was not rejected.
fn session(loaded: &Loaded) -> &SessionKeys {
    loaded.session.as_ref().expect("the file was rejected")
}

#[test]
fn a_session_s_keys_bind_and_unbind_c_c_sequences_over_its_defaults() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[keys.session]
"C-c g" = "sessions"
"C-c s" = false
"C-c x y" = "undo"
"C-c R" = { menu = "review" }
"#,
    );
    assert_eq!(messages(&loaded), Vec::<String>::new());
    let user = &session(&loaded).keys;
    assert_eq!(f.bound(user, "C-c g"), Some("sessions"));
    assert_eq!(f.bound(user, "C-c s"), Some("undefined"));
    assert_eq!(f.bound(user, "C-c x y"), Some("undo"));
    assert_eq!(f.bound(user, "C-c R"), Some("menu"));
    assert!(keys(&loaded).is_empty(), "the global keys are not touched");
}

#[test]
fn only_c_c_keys_reach_a_session_and_its_own_two_are_kept() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[keys.session]
"C-x g" = "undo"
a = false
"C-c" = "undo"
"C-c C-c" = "undo"
"C-c C-c x" = "undo"
"C-c C-k" = false
"C-c C-g" = "undo"
"C-c s x" = "undo"
"#,
    );
    let messages = messages(&loaded);
    assert_eq!(
        messages[..7],
        [
            r#"2:1: "C-x g": only C-c keys reach a session's keys"#,
            r#"3:1: "a": only C-c keys reach a session's keys"#,
            r#"4:1: "C-c": C-c alone begins a session's keys; put a key after it, such as "C-c g""#,
            r#"5:1: "C-c C-c": C-c C-c is not configurable in a session; it sends C-c to Claude Code"#,
            r#"6:1: "C-c C-c x": C-c C-c is not configurable in a session; it sends C-c to Claude Code"#,
            r#"7:1: "C-c C-k": C-c C-k is not configurable in a session; it ends char mode"#,
            r#"8:1: "C-c C-g": C-g is not configurable; it always cancels"#,
        ]
    );
    // `C-c s` runs `sessions` there, so it cannot become a prefix unless it is unbound, as
    // in `[keys.global]`.
    assert!(
        messages[7].starts_with(r#"9:1: "C-c s x": "#),
        "{}",
        messages[7]
    );
    assert_eq!(messages.len(), 8);
    assert!(session(&loaded).keys.is_empty());
}

#[test]
fn a_session_s_line_says_the_user_s_keys_over_the_defaults() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[keys.session]
"C-c g" = "sessions"
"C-c s" = false
"C-c x y" = "undo"
"C-c K" = "session-pin"
"#,
    );
    let help = session(&loaded).help.as_deref().unwrap();
    assert!(help.starts_with("C-c then:"), "{help}");
    for shown in ["  g sessions", "  x y undo", "  K pin", "  b switch"] {
        assert!(help.contains(shown), "{shown} in {help}");
    }
    for hidden in ["  s sessions", "  K stop", "  ? "] {
        assert!(!help.contains(hidden), "{hidden} in {help}");
    }
}

#[test]
fn without_session_keys_the_line_is_the_default_one() {
    let f = Fixture::new();
    let loaded = f.parse("[keys.global]\n\"C-c a\" = \"undo\"\n");
    let session = session(&loaded);
    assert!(session.keys.is_empty());
    let default = bindings::session_help(&f.session, &Keymap::new(), &f.registry, &f.menus);
    assert_eq!(session.help.as_deref(), Some(default.as_str()));
}

#[test]
fn a_removed_menu_s_session_key_is_undefined_and_left_out_of_the_line() {
    let f = Fixture::new();
    let loaded = f.parse("[menus]\nreview = false\n");
    let session = session(&loaded);
    assert_eq!(f.bound(&session.keys, "C-c V"), Some("undefined"));
    let help = session.help.as_deref().unwrap();
    assert!(!help.contains("review menu"), "{help}");
}

#[test]
fn a_long_session_table_ends_its_line_and_a_too_long_one_applies_nothing() {
    let f = Fixture::new();
    let mut text = String::from("[keys.session]\n");
    for n in 0..2_000 {
        let digits: Vec<String> = n.to_string().chars().map(String::from).collect();
        writeln!(text, "\"C-c y {} a\" = \"undo\"", digits.join(" ")).unwrap();
    }
    let loaded = f.parse(&text);
    let help = session(&loaded).help.as_deref().unwrap();
    assert!(
        help.len() <= bindings::MAX_HELP + "  …".len(),
        "{}",
        help.len()
    );
    assert!(help.ends_with("  …"), "{help}");
    let mut text = String::from("[keys.session]\n");
    for n in 0..=MAX_ENTRIES {
        writeln!(text, "\"C-c y {n}\" = \"undo\"").unwrap();
    }
    let loaded = f.parse(&text);
    assert!(session(&loaded).keys.is_empty());
    assert_eq!(
        messages(&loaded),
        [format!(
            "1:7: [keys.session] has {} entries, more than {MAX_ENTRIES}; none of its keys apply",
            MAX_ENTRIES + 1
        )]
    );
}
