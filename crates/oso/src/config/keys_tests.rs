//! The tests of the key rules of [`parse`](super::parse): what `[keys.global]` may bind and
//! unbind, what is refused and what warns, against the default keys (ADR 0108, ADR 0110).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::fmt::Write;

use oso_core::key::KeySequence;
use oso_core::keymap::Binding;

use super::keys::{MAX_ENTRIES, MAX_KEYS};
use super::tests::{Fixture, keys, messages};

#[test]
fn bad_entries_are_skipped_with_a_message_and_the_rest_applies() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[keys.global]
"C-c a" = "beginning-of-buffer"
"C-c b" = 1
"C-c c" = "no-such-command"
"C-c d" = true
"C-c foo" = "undo"
"C-c S-1" = "undo"
"" = "undo"
"C-c e" = ["undo"]
"#,
    );
    assert_eq!(
        messages(&loaded),
        [
            r#"3:11: "C-c b": a key takes a command's name or false, not a number"#,
            r#"4:11: "C-c c": no command is named "no-such-command""#,
            r#"5:11: "C-c d": true binds nothing; give a command's name, or false to unbind"#,
            r#"6:1: "C-c foo": "foo" is not a key; write keys as Emacs does, such as C-x, M-<f5> or SPC"#,
            r#"7:1: "C-c S-1": "S-1" is not a key; write keys as Emacs does, such as C-x, M-<f5> or SPC"#,
            r#"8:1: "": no keys"#,
            r#"9:11: "C-c e": a key takes a command's name or false, not a list"#,
        ]
    );
    let user = keys(&loaded);
    assert_eq!(f.bound(user, "C-c a"), Some("beginning-of-buffer"));
    assert_eq!(f.bound(user, "C-c b"), None);
}

#[cfg(not(feature = "dev"))]
#[test]
fn a_dev_builds_command_is_unknown_in_a_release_build() {
    let f = Fixture::new();
    let loaded = f.parse("[keys.global]\n\"C-c p\" = \"dev-panic\"\n");
    assert_eq!(
        messages(&loaded),
        [r#"2:11: "C-c p": no command is named "dev-panic""#]
    );
}

#[test]
fn other_keymaps_are_not_configurable_yet_and_the_question_never() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[keys.minibuffer]
"C-c a" = "undo"
[keys.completion]
[keys.isearch]
[keys.rust]
[keys.question]
y = "undo"
[keys."a b"]
"#,
    );
    assert_eq!(
        messages(&loaded),
        [
            "1:7: [keys.minibuffer] is not configurable yet; only [keys.global] is",
            "3:7: [keys.completion] is not configurable yet; only [keys.global] is",
            "4:7: [keys.isearch] is not configurable yet; only [keys.global] is",
            "5:7: [keys.rust] is not configurable yet; only [keys.global] is",
            "6:7: [keys.question] is never configurable: its keys answer the agent channel's \
             questions",
            r#"8:7: [keys."a b"] is not configurable yet; only [keys.global] is"#,
        ]
    );
    assert!(keys(&loaded).is_empty());
}

#[test]
fn c_g_is_refused_anywhere_in_a_sequence_with_shift_or_not() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[keys.global]
"C-g" = "undo"
"C-x C-g" = "undo"
"C-G" = false
"C-c C-S-g" = "undo"
"#,
    );
    let refused: Vec<usize> = loaded
        .problems
        .iter()
        .filter(|p| {
            p.message
                .contains("C-g is not configurable; it always cancels")
        })
        .map(|p| p.line)
        .collect();
    assert_eq!(refused, [2, 3, 4, 5], "{:?}", messages(&loaded));
    assert!(keys(&loaded).is_empty());
}

#[test]
fn a_sequence_cannot_start_with_a_typing_key_but_one_may_be_unbound() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[keys.global]
"a" = "undo"
"A" = "undo"
"SPC" = "undo"
"a b" = "undo"
"é" = "undo"
"q" = false
"s-a" = "undo"
"C-c a" = "undo"
"w x" = false
"SPC x" = false
"#,
    );
    assert_eq!(
        messages(&loaded),
        [
            r#"2:1: "a": a key that types a character cannot start a sequence; put it after a prefix, such as "C-c a""#,
            r#"3:1: "A": a key that types a character cannot start a sequence; put it after a prefix, such as "C-c A""#,
            r#"4:1: "SPC": a key that types a character cannot start a sequence; put it after a prefix, such as "C-c SPC""#,
            r#"5:1: "a b": a key that types a character cannot start a sequence; put it after a prefix, such as "C-c a b""#,
            r#"6:1: "é": a key that types a character cannot start a sequence; put it after a prefix, such as "C-c é""#,
            r#"10:1: "w x": a key that types a character cannot start a sequence; put it after a prefix, such as "C-c w x""#,
            r#"11:1: "SPC x": a key that types a character cannot start a sequence; put it after a prefix, such as "C-c SPC x""#,
        ]
    );
    let user = keys(&loaded);
    assert_eq!(f.bound(user, "q"), Some("undefined"));
    assert_eq!(
        f.bound(user, "w"),
        None,
        "an unbinding under it makes no prefix"
    );
    assert_eq!(f.bound(user, "s-a"), Some("undo"), "Super types nothing");
    assert_eq!(f.bound(user, "C-c a"), Some("undo"));
}

#[test]
fn a_command_over_a_default_prefix_applies_and_says_how_many_keys_it_hides() {
    let f = Fixture::new();
    let mut under = 0;
    let Some(Binding::Prefix(prefix)) = f.defaults.lookup(&"C-x".parse::<KeySequence>().unwrap().0)
    else {
        panic!("C-x is a default prefix");
    };
    prefix.bindings(|_, _| under += 1);
    assert!(under > 1);
    let loaded = f.parse("[keys.global]\nC-x = \"undo\"\n");
    assert_eq!(
        messages(&loaded),
        [format!(
            r#"2:1: "C-x": a prefix by default; this hides the {under} default keys under it"#
        )]
    );
    assert_eq!(f.bound(keys(&loaded), "C-x"), Some("undo"));
}

#[test]
fn a_command_over_a_prefix_the_file_unbinds_says_nothing_more() {
    let f = Fixture::new();
    let loaded = f.parse("[keys.global]\n\"C-x\" = false\n\"C-x 4\" = \"undo\"\n");
    assert_eq!(messages(&loaded), Vec::<String>::new());
    let user = keys(&loaded);
    assert_eq!(f.bound(user, "C-x 4"), Some("undo"));
    assert_eq!(f.bound(user, "C-x C-f"), Some("undefined"));
}

#[test]
fn a_prefix_over_a_default_command_needs_that_key_unbound() {
    let f = Fixture::new();
    let loaded = f.parse("[keys.global]\n\"C-a x\" = \"undo\"\n");
    assert_eq!(
        messages(&loaded),
        [
            r#"2:1: "C-a x": "C-a" runs mwim-beginning-of-code-or-line by default; add "C-a" = false to make it a prefix"#
        ]
    );
    assert!(keys(&loaded).is_empty());

    // In any order of the file: unbinding comes first.
    for text in [
        "[keys.global]\n\"C-a x\" = \"undo\"\n\"C-a\" = false\n\"C-a y\" = \"end-of-buffer\"\n",
        "[keys.global]\n\"C-a\" = false\n\"C-a y\" = \"end-of-buffer\"\n\"C-a x\" = \"undo\"\n",
    ] {
        let loaded = f.parse(text);
        assert_eq!(messages(&loaded), Vec::<String>::new());
        let user = keys(&loaded);
        assert_eq!(f.bound(user, "C-a x"), Some("undo"));
        assert_eq!(f.bound(user, "C-a y"), Some("end-of-buffer"));
        assert_eq!(f.bound(user, "C-a"), Some("prefix"));
    }
}

#[test]
fn unbinding_a_shifted_key_whose_unshifted_one_is_bound_warns() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[keys.global]
"C-F" = false
"S-<f5>" = false
"C-S-z" = false
"C-z" = false
"C-Q" = false
"C-X" = false
"C-c C-K" = false
"C-c C-k" = "undo"
"#,
    );
    assert_eq!(
        messages(&loaded),
        [
            r#"2:1: "C-F": changes nothing while "C-f" is bound, since a shifted key bound to nothing runs what it does without Shift"#,
            r#"3:1: "S-<f5>": changes nothing while "<f5>" is bound, since a shifted key bound to nothing runs what it does without Shift"#,
            r#"7:1: "C-X": changes nothing while "C-x" is bound, since a shifted key bound to nothing runs what it does without Shift"#,
            r#"8:1: "C-c C-K": changes nothing while "C-c C-k" is bound, since a shifted key bound to nothing runs what it does without Shift"#,
        ],
        "C-z is unbound by the file and C-q is bound by nothing"
    );
    // C-q has no default binding here; if it gains one, pick another key for the test.
    assert_eq!(
        f.bound(&f.defaults, "C-q"),
        None,
        "the test needs a key with no default"
    );
}

#[test]
fn of_entries_that_conflict_an_unbinding_wins_then_the_shorter_one() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[keys.global]
"C-c x a" = "undo"
"C-c x" = "end-of-buffer"
"C-c   y" = "undo"
"C-c y" = "end-of-buffer"
"C-c z a" = false
"C-c z" = "end-of-buffer"
"#,
    );
    assert_eq!(
        messages(&loaded),
        [
            r#"2:1: "C-c x a": "C-c x" is bound to a command here, so it cannot be a prefix; skipped"#,
            r#"5:1: "C-c y": these keys are set more than once; skipped"#,
            r#"7:1: "C-c z": other entries set keys after these; skipped"#,
        ]
    );
    let user = keys(&loaded);
    assert_eq!(f.bound(user, "C-c x"), Some("end-of-buffer"));
    assert_eq!(
        f.bound(user, "C-c y"),
        Some("undo"),
        "the first spelling in byte order"
    );
    assert_eq!(f.bound(user, "C-c z a"), Some("undefined"));

    // The file's order changes nothing: of two spellings, the later in byte order is skipped.
    let loaded = f.parse(
        "[keys.global]\n\"C-c y\" = \"end-of-buffer\"\n\"C-c   y\" = \"undo\"\n\"C-c z\" = \"end-of-buffer\"\n\"C-c z a\" = false\n",
    );
    assert_eq!(
        messages(&loaded),
        [
            r#"2:1: "C-c y": these keys are set more than once; skipped"#,
            r#"4:1: "C-c z": other entries set keys after these; skipped"#,
        ]
    );
    assert_eq!(f.bound(keys(&loaded), "C-c y"), Some("undo"));
}

#[test]
fn too_many_entries_bind_none_of_them() {
    let f = Fixture::new();
    let mut text = String::from("[keys.global]\n");
    for n in 0..=MAX_ENTRIES {
        let _ = writeln!(text, "\"C-c {n}\" = \"undo\"");
    }
    let loaded = f.parse(&text);
    assert_eq!(
        messages(&loaded),
        [format!(
            "1:7: [keys.global] has {} entries, more than {MAX_ENTRIES}; none of its keys apply",
            MAX_ENTRIES + 1
        )]
    );
    assert!(keys(&loaded).is_empty());
}

#[test]
fn a_sequence_has_at_most_max_keys() {
    let f = Fixture::new();
    let fits = format!("C-c{}", " a".repeat(MAX_KEYS - 1));
    let over = format!("C-d{}", " a".repeat(MAX_KEYS));
    let loaded = f.parse(&format!(
        "[keys.global]\n\"{fits}\" = \"undo\"\n\"{over}\" = \"undo\"\n\"C-d\" = false\n"
    ));
    assert_eq!(loaded.problems.len(), 1, "{:?}", messages(&loaded));
    assert!(
        loaded.problems[0]
            .message
            .ends_with(&format!("more than {MAX_KEYS} keys in one sequence"))
    );
    assert_eq!(f.bound(keys(&loaded), &fits), Some("undo"));
}

/// The default keys are read as the resolver reads them: a key bound to nothing is taken without
/// Shift, so `S-<left>` runs `<left>`'s command and `C-X` begins `C-x`'s keys.
#[test]
fn shifted_keys_shadow_the_defaults_they_reach_without_shift() {
    let f = Fixture::new();
    let mut under = 0;
    let Some(Binding::Prefix(prefix)) = f.defaults.lookup(&"C-x".parse::<KeySequence>().unwrap().0)
    else {
        panic!("C-x is a default prefix");
    };
    prefix.bindings(|_, _| under += 1);
    let cases = [
        (
            r#""S-<left> x" = "undo""#,
            r#"2:1: "S-<left> x": "S-<left>" runs backward-char by default; add "S-<left>" = false to make it a prefix"#.to_owned(),
        ),
        (
            r#""C-A x" = "undo""#,
            r#"2:1: "C-A x": "C-A" runs mwim-beginning-of-code-or-line by default; add "C-A" = false to make it a prefix"#.to_owned(),
        ),
        (
            r#""C-a x" = false"#,
            r#"2:1: "C-a x": "C-a" runs mwim-beginning-of-code-or-line by default; add "C-a" = false to make it a prefix"#.to_owned(),
        ),
        (
            r#""C-X 4" = "undo""#,
            format!(r#"2:1: "C-X 4": "C-X" begins the keys under "C-x" by default, without Shift; this makes it a prefix of its own and hides those {under} default keys"#),
        ),
        (
            r#""C-X C-F" = false"#,
            format!(r#"2:1: "C-X C-F": "C-X" begins the keys under "C-x" by default, without Shift; this makes it a prefix of its own and hides those {under} default keys"#),
        ),
        (
            r#""C-X" = "undo""#,
            format!(r#"2:1: "C-X": begins the keys under "C-x" by default, without Shift; this hides those {under} default keys"#),
        ),
    ];
    for (entry, message) in cases {
        let loaded = f.parse(&format!("[keys.global]\n{entry}\n"));
        assert_eq!(messages(&loaded), [message], "{entry}");
    }
    let user = |text: &str| f.parse(&format!("[keys.global]\n{text}\n"));
    let loaded = user(r#""S-<left> x" = "undo""#);
    assert!(keys(&loaded).is_empty(), "S-<left> still selects");
    let loaded = user(r#""C-a x" = false"#);
    assert!(keys(&loaded).is_empty(), "C-a still runs its command");

    // Each prefix is warned of once, and not at all where the file unbinds it.
    let loaded = user("\"C-X 4\" = \"undo\"\n\"C-X 5\" = \"undo\"");
    assert_eq!(loaded.problems.len(), 1, "{:?}", messages(&loaded));
    let loaded =
        user("\"C-X\" = false\n\"C-X 4\" = \"undo\"\n\"C-A\" = false\n\"C-A x\" = \"undo\"");
    assert_eq!(messages(&loaded), Vec::<String>::new());
    assert_eq!(f.bound(keys(&loaded), "C-A x"), Some("undo"));
}
