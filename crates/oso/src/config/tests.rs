//! The tests of [`parse`](super::parse) on the whole file: syntax errors, unknown names, the
//! settings, how a problem reads, and that no text makes it panic (ADR 0108, ADR 0110). The key
//! rules are in `keys_tests`.
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::fmt::Write;
use std::time::Duration;

use oso_core::command::Registry;
use oso_core::display_map::cell::escape;
use oso_core::jump::Keys;
use oso_core::key::KeySequence;
use oso_core::keymap::{Binding, Keymap};
use proptest::prelude::*;

use super::problems::{MAX_PROBLEMS, MAX_QUOTED};
use super::settings::{Aids, Jump, ThemeSetting};
use super::{Builtin, Loaded, STACK, parse};
use crate::{bindings, commands};

const THEMES: &[&str] = &["dark", "light", "dusk"];

/// The registry, default keymap and built-in menus that `parse` checks a file against.
pub(super) struct Fixture {
    pub(super) registry: Registry,
    pub(super) defaults: Keymap,
    pub(super) menus: oso_core::menu::Menus,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let registry = commands::registry();
        let defaults = bindings::global_keymap(&registry);
        let menus = bindings::built_in_menus(&registry);
        Self {
            registry,
            defaults,
            menus,
        }
    }

    pub(super) fn parse(&self, text: &str) -> Loaded {
        let builtin = Builtin {
            registry: &self.registry,
            defaults: &self.defaults,
            menus: &self.menus,
            themes: THEMES,
        };
        parse(text, builtin)
    }

    /// What `keys` runs in `keymap`: a command's name, "prefix", "undefined" or nothing.
    pub(super) fn bound(&self, keymap: &Keymap, keys: &str) -> Option<&'static str> {
        let keys: KeySequence = keys.parse().unwrap();
        Some(match keymap.lookup(&keys.0)? {
            Binding::Command(id) => self.registry.get(id).unwrap().name,
            Binding::Prefix(_) => "prefix",
            Binding::Menu(_) => "menu",
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

[save]
trim-whitespace = true

[search]
migemo = false
migemo-dictionary = "~/dict/migemo-dict"

[jump]
keys = "hjkl"
timeout = 0.3
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
        ThemeSetting {
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
    assert_eq!(settings.save.trim_whitespace, Some(true));
    assert_eq!(settings.search.migemo, Some(false));
    assert_eq!(
        settings.search.migemo_dictionary.as_deref(),
        Some("~/dict/migemo-dict")
    );
    assert_eq!(
        settings.jump,
        Jump {
            keys: Some(Keys::new("hjkl").unwrap()),
            timeout: Some(Duration::from_millis(300)),
        }
    );
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
[save]
trim = true
[jump]
labels = "asdf"
[search]
migemo-skk = "~/dict"
"#,
    );
    assert_eq!(
        messages(&loaded),
        [
            "1:2: unknown table [fonts], skipped",
            r#"4:1: unknown name "sparkles" in [aids], skipped"#,
            r#"7:1: unknown name "colour" in [theme], skipped"#,
            r#"9:1: unknown name "stop" in [agent-channel], skipped"#,
            r#"11:1: unknown name "trim" in [save], skipped"#,
            r#"13:1: unknown name "labels" in [jump], skipped"#,
            r#"15:1: unknown name "migemo-skk" in [search], skipped"#,
        ]
    );
    assert_eq!(loaded.settings.aids.flash, Some(true));
}

#[test]
fn a_table_that_is_a_value_is_reported() {
    let f = Fixture::new();
    let loaded = f.parse(
        "keys = 1\naids = \"on\"\n[agent-channel]\nstart = \"yes\"\n[save]\ntrim-whitespace = 1\n[search]\nmigemo = 0\n",
    );
    assert_eq!(
        messages(&loaded),
        [
            "1:8: [keys] must be a table",
            "2:8: [aids] must be a table",
            "4:9: [agent-channel] start takes true or false, not a string",
            "6:19: [save] trim-whitespace takes true or false, not a number",
            "8:10: [search] migemo takes true or false, not a number",
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
    // Quotes in the file's text cannot close the quotes around it.
    let loaded =
        f.parse("[keys.global]\n\"C-c a\" = 'x\" is fine; C-c a now runs \"undo'\n'C-c \\' = 1\n");
    assert_eq!(
        self::messages(&loaded),
        [
            r#"2:11: "C-c a": no command is named "x\" is fine; C-c a now runs \"undo""#,
            r#"3:11: "C-c \\": a key takes a command's name, { menu = "name" } or false, not a number"#,
        ]
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
    assert_eq!(loaded.settings.theme, ThemeSetting::default());
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
        Just("[jump]\n".to_owned()),
        Just("keys = \"asdf\"\n".to_owned()),
        Just("timeout = 0.5\n".to_owned()),
        Just("[[keys.global]]\n".to_owned()),
        Just("pair = [\"dusk\", \"light\"]\n".to_owned()),
        Just("default = \"dusk\"\n".to_owned()),
        Just("flash = true\n".to_owned()),
        Just("start = 1\n".to_owned()),
        Just("[menus.m]\n".to_owned()),
        Just("[[menus.m.groups]]\n".to_owned()),
        Just("menus.gone = false\n".to_owned()),
        Just("row = 0\n".to_owned()),
        Just("heads = [[\"a\", { \"\\u202e\\u001bx\" = 1 }, \"x\"]]\n".to_owned()),
        Just("heads = [[\"a b\", { menu = \"m\" }, \"l\", { count = 99999 }]]\n".to_owned()),
        Just("\"C-c m\" = { menu = \"m\" }\n".to_owned()),
        "heads = \\[\\[\"[-CMa-z ]{0,8}\", (\"[a-z-]{0,12}\"|false|\\{ menu = \"m\" \\}|\\{ switch = \"[a-z]{0,4}\" \\}), \"[ -~]{0,8}\"\\]\\]\n",
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
        prop_assert!(loaded.problems.len() <= MAX_PROBLEMS);
        for problem in &loaded.problems {
            prop_assert!(problem.line >= 1 && problem.column >= 1);
            prop_assert!(
                problem.message.chars().all(|c| escape(c).is_none()),
                "{}", problem.message
            );
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

/// Tables nest only so deep: the parser refuses a key of more than 80 parts, dotted keys and table
/// headers too, and nesting of inline tables and arrays past 80.
#[test]
fn keys_and_nesting_have_limits() {
    let f = Fixture::new();
    for text in [
        format!("{} = 1\n", vec!["a"; 81].join(".")),
        format!("[{}]\n", vec!["b"; 81].join(".")),
        format!("[[{}]]\n", vec!["c"; 81].join(".")),
    ] {
        assert_eq!(
            messages(&f.parse(&text)),
            ["1:1: syntax error: recursion limit"]
        );
    }
}

/// The deepest tables a file can make, an 80-part dotted key in each of 79 nested inline tables,
/// parse and go: on a small stack when the file parses, as `parse` drops them a level at a time,
/// and within [`STACK`] when it does not, where the parser drops what it built itself.
#[test]
fn the_deepest_tables_parse_and_drop_within_the_stack_parse_needs() {
    // Dropping them recursively takes megabytes; a level at a time, under 128 KiB.
    const SMALL: usize = 256 * 1024;
    let path = vec!["d"; 80].join(".");
    let deepest = format!(
        "[{path}]\n{path} = {}1{}\n",
        format!("{{ {path} = ").repeat(79),
        " }".repeat(79)
    );
    let cases = [
        (SMALL, deepest.clone(), "1:2: unknown table [d], skipped"),
        (STACK, format!("{deepest}= 1\n"), "3:1: syntax error: "),
        (
            STACK,
            format!("{}{deepest}= 1\n", "# padding\n".repeat(7000)),
            "7003:1: syntax error: ",
        ),
    ];
    for (stack, text, first) in cases {
        let messages = std::thread::Builder::new()
            .stack_size(stack)
            .spawn(move || messages(&Fixture::new().parse(&text)))
            .unwrap()
            .join()
            .unwrap();
        assert!(messages[0].starts_with(first), "{messages:?}");
    }
}

#[test]
fn only_the_first_problems_in_the_file_are_kept_and_the_rest_counted() {
    let f = Fixture::new();
    let mut text = String::from("[aids]\n");
    for n in (0..1000).rev() {
        let _ = writeln!(text, "x{n:04} = 1");
    }
    let loaded = f.parse(&text);
    assert_eq!(loaded.problems.len(), MAX_PROBLEMS);
    assert_eq!(loaded.more, 1000 - MAX_PROBLEMS);
    let lines: Vec<usize> = loaded.problems.iter().map(|p| p.line).collect();
    assert_eq!(lines, (2..2 + MAX_PROBLEMS).collect::<Vec<_>>());

    let errors = "= 1\n".repeat(1000);
    assert!(errors.len() <= 64 * 1024);
    let loaded = f.parse(&errors);
    assert!(loaded.rejected());
    assert_eq!(loaded.problems.len(), MAX_PROBLEMS);
    assert_eq!(loaded.problems[0].line, 1);
    assert!(loaded.more > 0);
}

#[test]
fn a_large_file_reports_only_its_first_syntax_error() {
    let f = Fixture::new();
    let mut text = "# padding\n".repeat(64 * 1024 / 10);
    text.push_str("= 1\n= 2\n");
    let line = 64 * 1024 / 10 + 1;
    let loaded = f.parse(&text);
    assert!(loaded.rejected());
    assert_eq!(
        messages(&loaded),
        [
            format!(
                "{line}:1: syntax error: unquoted keys cannot be empty, expected letters, \
                 numbers, `-`, `_`"
            ),
            format!("{line}:1: a file over 64 KiB reports only its first syntax error"),
        ]
    );
}

#[test]
fn indentation_is_set_per_mode_and_bad_values_are_reported() {
    use oso_core::indent::Style;

    use super::indentation::ModeIndent;

    let f = Fixture::new();
    let loaded = f.parse(
        r#"
[indent]
electric = false

[modes.rust]
indent-size = 2
indent-style = "tab"
tab-width = 4

[modes.markdown]
indent-size = 0
indent-style = "tabs"
tab-width = 33
wrap = true

[modes.python]
indent-size = 4
"#,
    );
    assert_eq!(
        messages(&loaded),
        [
            "11:15: [modes.markdown] indent-size takes a number from 1 to 32, not 0",
            "12:16: [modes.markdown] indent-style takes \"space\" or \"tab\", not \"tabs\"",
            "13:13: [modes.markdown] tab-width takes a number from 1 to 32, not 33",
            "14:1: unknown name \"wrap\" in [modes.markdown], skipped",
            "16:8: unknown name \"python\" in [modes], skipped",
        ]
    );
    let settings = &loaded.settings;
    assert_eq!(settings.indent.electric, Some(false));
    assert_eq!(
        settings.modes[0],
        ModeIndent {
            indent_size: Some(2),
            style: Some(Style::Tabs),
            tab_width: Some(4),
        }
    );
    assert_eq!(settings.modes[1], ModeIndent::default());
}

#[test]
fn an_indentation_width_in_another_form_is_reported_and_not_used() {
    let f = Fixture::new();
    let loaded = f.parse(
        "[modes.toml]\nindent-size = 0x8\ntab-width = -4\n[modes.rust]\nindent-size = 2.0\n",
    );
    assert_eq!(
        messages(&loaded),
        [
            "2:15: [modes.toml] indent-size takes a number from 1 to 32, not 8",
            "3:13: [modes.toml] tab-width takes a number from 1 to 32, not -4",
            "5:15: [modes.rust] indent-size takes a number from 1 to 32, not a number",
        ]
    );
    assert_eq!(
        loaded.settings.modes,
        [super::indentation::ModeIndent::default(); 4]
    );
}
