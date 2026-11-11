//! The tests of [`parse`](super::parse) on the whole file: syntax errors, unknown names, the
//! settings, how a problem reads, and that no text makes it panic (ADR 0108, ADR 0110). The key
//! rules are in `keys_tests`.
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::fmt::Write;

use oso_core::command::Registry;
use oso_core::key::KeySequence;
use oso_core::keymap::{Binding, Keymap};
use proptest::prelude::*;

use super::problems::MAX_QUOTED;
use super::settings::{Aids, Theme};
use super::{Loaded, parse};
use crate::{bindings, commands};

const THEMES: &[&str] = &["dark", "light", "dusk"];

/// The registry and default keymap that `parse` checks a file against.
pub(super) struct Fixture {
    pub(super) registry: Registry,
    pub(super) defaults: Keymap,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let registry = commands::registry();
        let defaults = bindings::global_keymap(&registry);
        Self { registry, defaults }
    }

    pub(super) fn parse(&self, text: &str) -> Loaded {
        parse(text, &self.registry, &self.defaults, THEMES)
    }

    /// What `keys` runs in `keymap`: a command's name, "prefix", "undefined" or nothing.
    pub(super) fn bound(&self, keymap: &Keymap, keys: &str) -> Option<&'static str> {
        let keys: KeySequence = keys.parse().unwrap();
        Some(match keymap.lookup(&keys.0)? {
            Binding::Command(id) => self.registry.get(id).unwrap().name,
            Binding::Prefix(_) => "prefix",
            Binding::Undefined => "undefined",
        })
    }
}

/// Each problem as `line:column: message`.
pub(super) fn messages(loaded: &Loaded) -> Vec<String> {
    loaded.problems.iter().map(ToString::to_string).collect()
}

/// The user keymap of a file that was not rejected.
pub(super) fn keys(loaded: &Loaded) -> &Keymap {
    loaded.keys.as_ref().expect("the file was rejected")
}

#[test]
fn an_empty_file_binds_nothing_and_sets_nothing() {
    let f = Fixture::new();
    let loaded = f.parse("");
    assert!(!loaded.rejected());
    assert!(keys(&loaded).is_empty());
    assert_eq!(loaded.settings, super::Settings::default());
    assert!(loaded.problems.is_empty());
}

#[test]
fn a_file_binds_unbinds_and_sets() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"
[keys.global]
"C-c a" = "beginning-of-buffer"
C-z = false
"<f5>" = "load-theme"

[theme]
default = "dusk"
pair = ["dusk", "light"]

[aids]
whitespace = true
current-line = false
flash = true
line-numbers = false

[agent-channel]
start = true
"#,
    );
    assert_eq!(messages(&loaded), Vec::<String>::new());
    let user = keys(&loaded);
    assert_eq!(f.bound(user, "C-c a"), Some("beginning-of-buffer"));
    assert_eq!(f.bound(user, "C-z"), Some("undefined"));
    assert_eq!(f.bound(user, "<f5>"), Some("load-theme"));
    let settings = &loaded.settings;
    assert_eq!(
        settings.theme,
        Theme {
            default: Some("dusk".to_owned()),
            pair: Some(["dusk".to_owned(), "light".to_owned()]),
        }
    );
    assert_eq!(
        settings.aids,
        Aids {
            whitespace: Some(true),
            current_line: Some(false),
            flash: Some(true),
            line_numbers: Some(false),
        }
    );
    assert_eq!(settings.agent_channel_start, Some(true));
}

#[test]
fn a_syntax_error_rejects_the_whole_file_and_every_one_is_reported() {
    let f = Fixture::new();
    let loaded =
        f.parse("[keys.global]\n\"C-c a\" = \"beginning-of-buffer\"\n= 1\n[aids]\nflash = tru\n");
    assert!(loaded.rejected());
    assert_eq!(loaded.settings, super::Settings::default());
    let lines: Vec<usize> = loaded.problems.iter().map(|p| p.line).collect();
    assert!(
        lines.contains(&3) && lines.contains(&5),
        "{:?}",
        messages(&loaded)
    );
    assert!(
        loaded
            .problems
            .iter()
            .all(|p| p.message.starts_with("syntax error: ")),
        "{:?}",
        messages(&loaded)
    );
}

#[test]
fn a_key_defined_twice_is_a_syntax_error() {
    let f = Fixture::new();
    let loaded = f.parse("[keys.global]\nC-z = false\nC-z = \"undo\"\n");
    assert!(loaded.rejected());
    assert_eq!(loaded.problems.len(), 1);
    assert_eq!(loaded.problems[0].line, 3);
}

#[test]
fn unknown_tables_and_names_warn_and_are_skipped() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[fonts]
size = 12
[aids]
sparkles = true
flash = true
[theme]
colour = "dusk"
[agent-channel]
stop = true
"#,
    );
    assert_eq!(
        messages(&loaded),
        [
            "1:2: unknown table [fonts], skipped",
            r#"4:1: unknown name "sparkles" in [aids], skipped"#,
            r#"7:1: unknown name "colour" in [theme], skipped"#,
            r#"9:1: unknown name "stop" in [agent-channel], skipped"#,
        ]
    );
    assert_eq!(loaded.settings.aids.flash, Some(true));
}

#[test]
fn a_table_that_is_a_value_is_reported() {
    let f = Fixture::new();
    let loaded = f.parse("keys = 1\naids = \"on\"\n[agent-channel]\nstart = \"yes\"\n");
    assert_eq!(
        messages(&loaded),
        [
            "1:8: [keys] must be a table",
            "2:8: [aids] must be a table",
            "4:9: [agent-channel] start takes true or false, not a string",
        ]
    );
}

#[test]
fn quoted_text_is_cut_and_escaped() {
    let f = Fixture::new();
    let long = "x".repeat(200);
    let loaded = f.parse(&format!(
        "[keys.global]\n\"C-c a\" = \"{long}\"\n\"\\u202E\" = \"undo\"\n\"C-c \\u001b\" = \"undo\"\n"
    ));
    let messages = messages(&loaded);
    assert_eq!(
        messages[0],
        format!(
            r#"2:11: "C-c a": no command is named "{}…""#,
            "x".repeat(MAX_QUOTED)
        )
    );
    assert!(
        messages[1].starts_with(r#"3:1: "U+202E": "#),
        "{}",
        messages[1]
    );
    assert!(
        messages[2].starts_with(r#"4:1: "C-c ^[": "^[" is not a key"#),
        "{}",
        messages[2]
    );
    assert!(
        messages.iter().all(|m| !m.contains(['\u{202e}', '\u{1b}'])),
        "{messages:?}"
    );
}

#[test]
fn a_problem_has_its_line_and_column_in_characters() {
    let f = Fixture::new();
    let loaded = f.parse("[aids]\n  flash = 1\nwhitespace = true\n[theme] \n  \"é\" = \"x\"\n");
    assert_eq!(
        messages(&loaded),
        [
            "2:11: [aids] flash takes true or false, not a number",
            r#"5:3: unknown name "é" in [theme], skipped"#,
        ]
    );
    let loaded = f.parse("[aids]\nflash = 1 # éé\n\"éé\" = true\n");
    assert_eq!(loaded.problems[1].column, 1);
}

#[test]
fn themes_are_built_in_ones_and_a_pair_is_two_different_ones() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"[theme]
default = "solarized"
pair = ["dusk", "dusk"]
"#,
    );
    assert_eq!(
        messages(&loaded),
        [
            r#"2:11: [theme] default: no built-in theme is named "solarized""#,
            r#"3:8: [theme] pair names "dusk" twice"#,
        ]
    );
    assert_eq!(loaded.settings.theme, Theme::default());
    for pair in [r#"["dusk"]"#, r#""dusk""#, r#"["dusk", "light", "dark"]"#] {
        let loaded = f.parse(&format!("[theme]\npair = {pair}\n"));
        assert_eq!(
            messages(&loaded),
            [r#"2:8: [theme] pair takes a list of two themes' names, such as ["dusk", "light"]"#]
        );
    }
    let loaded = f.parse("[theme]\npair = [\"dusk\", 1]\ndefault = true\n");
    assert_eq!(
        messages(&loaded),
        [
            "2:17: [theme] pair takes a theme's name, not a number",
            "3:11: [theme] default takes a theme's name, not true or false",
        ]
    );
}

/// Text that looks like a configuration file: tables, keys and values, some of them wrong.
fn configish() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("[keys.global]\n".to_owned()),
        Just("[keys]\n".to_owned()),
        Just("[theme]\n".to_owned()),
        Just("[aids]\n".to_owned()),
        Just("[agent-channel]\n".to_owned()),
        Just("[[keys.global]]\n".to_owned()),
        Just("pair = [\"dusk\", \"light\"]\n".to_owned()),
        Just("default = \"dusk\"\n".to_owned()),
        Just("flash = true\n".to_owned()),
        Just("start = 1\n".to_owned()),
        "\"[-CMSs a-zA-Z0-9<>gx ]{0,12}\" = (\"[a-z-]{0,20}\"|false|true|1|\\[\\])\n",
        "[ -~\n]{0,20}",
        any::<String>(),
    ];
    prop::collection::vec(piece, 0..24).prop_map(|pieces| pieces.concat())
}

proptest! {
    #[test]
    fn parse_never_panics(text in prop_oneof![any::<String>(), configish()]) {
        let f = Fixture::new();
        let loaded = f.parse(&text);
        prop_assert!(loaded.problems.windows(2).all(|w| (w[0].line, w[0].column) <= (w[1].line, w[1].column)));
        for problem in &loaded.problems {
            prop_assert!(problem.line >= 1 && problem.column >= 1);
            prop_assert!(!problem.message.chars().any(|c| c.is_control()));
        }
    }
}

/// Keys that type a character, for sequences under a prefix.
const KEYS: &[char] = &[
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
];

/// A 64 KiB file parses and builds within ADR 0108's budget of 2 ms on the reading thread. Run
/// it by hand in release: `cargo test --release -p oso config::tests::a_64_kib -- --ignored`.
#[test]
#[ignore = "a timing to read, run by hand in release"]
fn a_64_kib_file_parses_and_builds_in_under_2_ms() {
    use std::time::Instant;
    let f = Fixture::new();
    let mut text = String::from("[keys.global]\n");
    let mut n = 0;
    while text.len() < 64 * 1024 {
        let (first, second) = (KEYS[n / KEYS.len() % KEYS.len()], KEYS[n % KEYS.len()]);
        let _ = writeln!(
            text,
            "\"C-c {} {first} {second}\" = \"undo\"",
            n / KEYS.len().pow(2)
        );
        n += 1;
    }
    let mut best = std::time::Duration::MAX;
    for _ in 0..20 {
        let start = Instant::now();
        let loaded = f.parse(&text);
        let took = start.elapsed();
        assert!(!loaded.rejected());
        assert_eq!(loaded.problems, []);
        drop(loaded);
        best = best.min(took);
    }
    // A debug build is several times slower; the budget is a release build's.
    assert!(
        cfg!(debug_assertions) || best.as_micros() < 2000,
        "{n} entries, {} bytes: {best:?}",
        text.len()
    );
}
