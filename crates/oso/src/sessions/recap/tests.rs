//! The recaps of the bundled rules over the recorded screens, the title they fall back to, and
//! how a recap is cleaned and cut.
//!
//! Must not depend on: `app` or `dispatch`; these tests drive the sessions' own
//! parts, with no window and no terminal.

use std::collections::HashSet;
use std::time::Duration;

use super::{MAX_CHARS, clean, recap};
use crate::sessions::engine::evaluate;
use crate::sessions::rules::{Deadline, RuleSet};
use crate::sessions::screen::Evidence;

const TITLE: &str = "✳ Date to now.txt";

/// The recap of a recorded screen, under the bundled rules.
fn of(text: &str) -> Option<String> {
    let rules = RuleSet::bundled();
    let evidence = Evidence::new(text.lines(), TITLE, None);
    let verdict = evaluate(&rules, &evidence);
    let deadline = Deadline::after(Duration::from_secs(5));
    recap(&rules, &verdict, &evidence, &HashSet::new(), &deadline)
}

#[test]
fn a_dialog_is_recapped_by_its_question() {
    let cases = [
        (
            include_str!("../screens/permission-bash.txt"),
            "Do you want to proceed?",
        ),
        (
            include_str!("../screens/question.txt"),
            "Which colour do you prefer, red or blue?",
        ),
        (
            include_str!("../screens/plan-approval.txt"),
            "Plan: append \"bye\" to README.md",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(of(text).as_deref(), Some(expected));
    }
    let elicitation = "\
────────────────────────
 MCP server \"tracker\" requests your input
 ❯ Accept
   Decline
 Esc to cancel
";
    assert_eq!(
        of(elicitation).as_deref(),
        Some("MCP server \"tracker\" requests your input")
    );
}

#[test]
fn a_turn_is_recapped_by_its_status_line_whichever_working_rule_matched() {
    let working = include_str!("../screens/working.txt");
    assert_eq!(of(working).as_deref(), Some("Nucleating…"));
    let planning = include_str!("../screens/working-plan-mode.txt");
    assert_eq!(of(planning).as_deref(), Some("Zigzagging…"));
    let footless = working.replace("· esc to interrupt", "");
    assert_eq!(of(&footless).as_deref(), Some("Nucleating…"));
}

#[test]
fn otherwise_the_title_and_a_held_screen_keeps_the_last_recap() {
    for text in [
        include_str!("../screens/done.txt"),
        include_str!("../screens/start-trust.txt"),
        include_str!("../screens/transcript-viewer.txt"),
    ] {
        assert_eq!(of(text).as_deref(), Some(TITLE));
    }
    assert_eq!(of(include_str!("../screens/help.txt")), None, "hold");
}

#[test]
fn a_late_recap_keeps_the_last_one() {
    let rules = RuleSet::bundled();
    let evidence = Evidence::new(include_str!("../screens/working.txt").lines(), TITLE, None);
    let verdict = evaluate(&rules, &evidence);
    let late = Deadline::after(Duration::ZERO);
    assert_eq!(
        recap(&rules, &verdict, &evidence, &HashSet::new(), &late),
        None
    );
}

#[test]
fn a_recap_is_one_clean_line_cut_to_its_length() {
    assert_eq!(clean("  a \t b\u{7}c  "), "a b c");
    assert_eq!(clean("x\u{202E}cod.exe\u{2066}y\u{200F}"), "xcod.exey");
    assert_eq!(clean("\u{202E}  \u{2069}"), "");
    let fits = "a".repeat(MAX_CHARS);
    assert_eq!(clean(&fits), fits);
    let long = "é".repeat(MAX_CHARS + 1);
    let cut = clean(&long);
    assert_eq!(cut.chars().count(), MAX_CHARS);
    assert!(cut.ends_with("é…"));
    let spaced = format!("{} b", "a".repeat(MAX_CHARS - 1));
    assert_eq!(clean(&spaced), format!("{}…", "a".repeat(MAX_CHARS - 1)));
}

#[test]
fn the_titles_spinner_is_no_part_of_the_recap() {
    let rules = RuleSet::bundled();
    let deadline = Deadline::after(Duration::from_secs(5));
    let screen = include_str!("../screens/done.txt");
    let recaps: Vec<Option<String>> = ["\u{2802} Date to now.txt", "\u{2810} Date to now.txt"]
        .into_iter()
        .map(|title| {
            let evidence = Evidence::new(screen.lines(), title, None);
            let verdict = evaluate(&rules, &evidence);
            recap(&rules, &verdict, &evidence, &HashSet::new(), &deadline)
        })
        .collect();
    let done = Some("Date to now.txt".to_owned());
    assert_eq!(recaps, [done.clone(), done]);
}
