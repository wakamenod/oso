//! The rules engine's fuzz target (ADR 0121): rules files and screens made at random, from
//! parts that reach every branch of the format and the regions, and from arbitrary text.
//! Whatever they are, parsing and judging never panic, keep within their limits, and say what
//! they matched truly.
//!
//! Must not depend on: `app`, `commands` or `dispatch`; these tests drive the sessions' own
//! parts, with no window and no terminal.
//!
//! It runs in `just ci` with proptest's default cases. For a long run:
//! `PROPTEST_CASES=200000 cargo test -p oso --release sessions::fuzz_tests`.
//!
//! The tip crate has no library target for cargo-fuzz to link, so this is proptest's; a
//! cargo-fuzz harness can share these generators once one exists (ADR 0120's terminal).

use proptest::prelude::*;

use super::engine::evaluate;
use super::explain;

use super::rules::{MAX_PROBLEMS, MAX_RULES, RuleSet, Source, parse};
use super::screen::{Evidence, MAX_ROW_BYTES, MAX_ROWS, MAX_TITLE_BYTES, Progress};

/// Text that looks like the TUI's: its marks, rules, dialogs' words, wide and odd characters.
fn screen_text() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z ]{0,30}",
        Just("────────────────".to_owned()),
        Just("❯ ".to_owned()),
        Just(" ❯ 1. Yes".to_owned()),
        Just("Do you want to proceed?".to_owned()),
        Just("Enter to select · ↑/↓ to navigate · Esc to cancel".to_owned()),
        Just("Enter to confirm · Esc to cancel".to_owned()),
        Just("  ⏸ manual mode on · esc to interrupt".to_owned()),
        Just("✶ Nucleating…".to_owned()),
        Just("4. Chat about this".to_owned()),
        Just("showing detailed transcript".to_owned()),
        "\\PC{0,20}",
        any::<String>(),
        "[─╌é\u{7}\u{1b}\t]{0,2000}",
    ]
}

/// A pattern: ordinary, unicode-heavy, invalid, or meant to be costly.
fn pattern() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z]{1,8}",
        Just(r"^\s*❯".to_owned()),
        Just(r"(?i)esc\s+to\s+cancel\s*$".to_owned()),
        Just(r"(a*)*b".to_owned()),
        Just(r"(?s).*.*.*x".to_owned()),
        Just(r"\w{1,200}\W{1,200}".to_owned()),
        Just(r"(?:\w{50}){50}".to_owned()),
        Just(r"[\x{2800}-\x{28FF}]".to_owned()),
        Just("(".to_owned()),
        Just(r"\".to_owned()),
        "\\PC{0,12}",
    ]
}

/// A TOML string literal of `text`, with no way out of its quotes.
fn literal(text: &str) -> String {
    let escaped: String = text
        .chars()
        .map(|c| match c {
            '\\' => "\\\\".to_owned(),
            '"' => "\\\"".to_owned(),
            c if c.is_control() => format!("\\u{:04X}", u32::from(c)),
            c => c.to_string(),
        })
        .collect();
    format!("\"{escaped}\"")
}

/// A test's matchers, nested up to `depth` more levels: deeper than the limit, sometimes.
fn matchers(depth: u32) -> BoxedStrategy<String> {
    let leaf = prop_oneof![
        prop::collection::vec(screen_text(), 0..3).prop_map(|parts| {
            let parts: Vec<String> = parts.iter().map(|p| literal(p)).collect();
            format!("contains = [{}]", parts.join(", "))
        }),
        prop::collection::vec(pattern(), 1..3).prop_map(|parts| {
            let parts: Vec<String> = parts.iter().map(|p| literal(p)).collect();
            format!("regex = [{}]", parts.join(", "))
        }),
        pattern().prop_map(|p| format!("line_regex = [{}]", literal(&p))),
        Just("unknown = 1".to_owned()),
    ];
    if depth == 0 {
        return leaf.boxed();
    }
    let parts = (
        prop_oneof![Just("any"), Just("all"), Just("not")],
        prop::collection::vec(matchers(depth - 1), 0..3),
    );
    let nested = parts.prop_map(|(key, inner)| {
        let tables: Vec<String> = inner.iter().map(|m| format!("{{ {m} }}")).collect();
        format!("{key} = [{}]", tables.join(", "))
    });
    prop_oneof![3 => leaf, 1 => nested].boxed()
}

/// One `[[rule]]`, its fields right or wrong.
fn rule() -> impl Strategy<Value = String> {
    let parts = (
        prop_oneof![Just("a"), Just("b"), Just("c"), Just("bad name"), Just("")],
        prop_oneof![
            Just("state = \"needs_input\"\nreason = \"permission\""),
            Just("state = \"needs_input\""),
            Just("state = \"working\""),
            Just("state = \"idle\"\nvisible = true"),
            Just("state = \"hold\""),
            Just("state = \"done\""),
            Just("disable = true"),
        ],
        prop_oneof![
            Just("title"),
            Just("progress"),
            Just("bottom(3)"),
            Just("bottom(40)"),
            Just("bottom(99)"),
            Just("prompt_box"),
            Just("after_last_rule"),
            Just("above_prompt_box"),
        ],
        -200_000i64..200_000,
        prop::collection::vec(matchers(6), 0..4),
    );
    parts.prop_map(|(name, state, region, priority, tests)| {
        format!(
            "[[rule]]\nname = {}\n{state}\nregion = \"{region}\"\npriority = {priority}\n{}\n",
            literal(name),
            tests.join("\n")
        )
    })
}

fn rules_file() -> impl Strategy<Value = String> {
    let parts = (
        prop_oneof![8 => Just("version = 1\n"), 1 => Just("version = 9\n"), 1 => Just("")],
        prop::collection::vec(rule(), 0..8),
    );
    parts.prop_map(|(version, rules)| format!("{version}{}", rules.concat()))
}

fn evidence() -> impl Strategy<Value = Evidence> {
    let parts = (
        prop::collection::vec(screen_text(), 0..60),
        screen_text(),
        prop::option::of((0u8..6, prop::option::of(any::<u8>()))),
        any::<bool>(),
    );
    parts.prop_map(|(rows, title, progress, echo_off)| {
        let progress = progress.and_then(|(state, value)| Progress::new(state, value));
        if echo_off {
            Evidence::echo_off(&title, progress)
        } else {
            Evidence::new(rows.iter().map(String::as_str), &title, progress)
        }
    })
}

/// Parses `local` over the bundled rules, judges `evidence`, and checks every bound.
fn check(local: &str, evidence: &Evidence) -> Result<(), TestCaseError> {
    let parsed = parse(local, Source::Local);
    prop_assert!(parsed.rules.len() <= MAX_RULES);
    prop_assert!(parsed.problems.len() <= MAX_PROBLEMS);
    if parsed.refused {
        prop_assert!(parsed.rules.is_empty() && parsed.disabled.is_empty());
    }
    let rules = RuleSet::merged(super::rules::bundled(), parsed);
    prop_assert!(evidence.rows().len() <= MAX_ROWS);
    for row in evidence.rows() {
        prop_assert!(row.len() <= MAX_ROW_BYTES && !row.is_empty());
        prop_assert!(!row.chars().any(char::is_control));
    }
    let verdict = evaluate(&rules, evidence);
    if let Ok(matched) = &verdict.by {
        prop_assert_eq!(&matched.text, &evidence.region(matched.region));
        let rule = rules.rules().iter().find(|rule| rule.name == matched.name);
        prop_assert!(rule.is_some_and(|rule| rule.target == verdict.target));
    }
    let shown = explain::text("fuzz", &rules, evidence, &verdict);
    let bound = MAX_ROWS * (MAX_ROW_BYTES + 8) + MAX_TITLE_BYTES + 1024;
    prop_assert!(shown.len() <= bound, "{} bytes", shown.len());
    Ok(())
}

proptest! {
    #[test]
    fn made_rules_over_made_screens_stay_within_bounds(local in rules_file(), evidence in evidence()) {
        check(&local, &evidence)?;
    }

    #[test]
    fn any_text_as_a_rules_file_is_refused_or_read(text in any::<String>(), evidence in evidence()) {
        check(&text, &evidence)?;
    }
}
