//! The rules files' format, its limits and what each problem says.
//!
//! Must not depend on: `app`, `commands` or `dispatch`; these tests drive the sessions' own
//! parts, with no window and no terminal.

use std::fmt::Write as _;
use std::time::Duration;

use super::parse::MAX_PROBLEMS;
use super::{
    Deadline, Late, MAX_DEPTH, MAX_MATCHERS, MAX_PATTERNS, MAX_RULES, Parsed, RuleSet, Source,
    bundled, parse,
};
use crate::sessions::state::{Reason, Target};

fn local(text: &str) -> Parsed {
    parse(text, Source::Local)
}

/// One rule with `body` after its name, in a file of version 1.
fn one(body: &str) -> Parsed {
    local(&format!("version = 1\n[[rule]]\nname = \"r\"\n{body}\n"))
}

const GOOD: &str = "state = \"working\"\nregion = \"title\"\ncontains = [\"x\"]";

#[test]
fn the_bundled_rules_have_no_problems() {
    let parsed = bundled();
    assert_eq!(parsed.problems, Vec::<String>::new());
    assert!(!parsed.refused);
    assert!(parsed.rules.len() >= 10);
    let set = RuleSet::bundled();
    let priorities: Vec<i64> = set.rules().iter().map(|rule| rule.priority).collect();
    assert!(priorities.windows(2).all(|pair| pair[0] >= pair[1]));
}

#[test]
fn a_good_rule_takes_every_field() {
    let parsed = one(
        "state = \"needs_input\"\nreason = \"plan\"\npriority = -5\nregion = \"bottom(7)\"\n\
         contains = \"Yes\"\nregex = ['a']\nline_regex = ['b', 'c']\n\
         any = [{ contains = [\"d\"] }]\nall = [{ regex = ['e'] }]\nnot = [{ line_regex = ['f'] }]",
    );
    assert_eq!(parsed.problems, Vec::<String>::new());
    let rule = &parsed.rules[0];
    assert_eq!(rule.target, Target::NeedsInput(Reason::Plan));
    assert_eq!(rule.priority, -5);
    assert_eq!(rule.source, Source::Local);
    let deadline = Deadline::after(Duration::from_secs(5));
    let holds = |text: &str| rule.holds(text, &text.to_ascii_lowercase(), &deadline);
    assert_eq!(holds("Yes a\nb\nc d e"), Ok(true));
    assert_eq!(holds("Yes a\nb\nc d e\nf"), Ok(false));
    let late = Deadline::after(Duration::ZERO);
    assert_eq!(rule.holds("Yes", "yes", &late), Err(Late));
}

#[test]
fn a_file_is_refused_for_its_syntax_or_its_version() {
    for text in [
        "version = 1\n[[rule]\n",
        "[[rule]]\nname = \"r\"\nstate = \"idle\"\nregion = \"title\"\ncontains = [\"x\"]",
        "version = 2",
        "version = \"1\"",
    ] {
        let parsed = local(text);
        assert!(parsed.refused, "{text}");
        assert!(parsed.rules.is_empty());
        assert_eq!(parsed.problems.len(), 1, "{text}: {:?}", parsed.problems);
    }
}

/// A rule's body after its name, each wrong in one way, and what its problem says.
const WRONG: &[(&str, &str)] = &[
    ("region = \"title\"\ncontains = [\"x\"]", "it has no state"),
    (
        "state = \"busy\"\nregion = \"title\"\ncontains = [\"x\"]",
        "state must be",
    ),
    (
        "state = \"needs_input\"\nregion = \"title\"\ncontains = [\"x\"]",
        "needs a reason",
    ),
    (
        "state = \"needs_input\"\nreason = \"x\"\nregion = \"title\"\ncontains = [\"x\"]",
        "reason must be",
    ),
    (
        "state = \"working\"\nreason = \"plan\"\nregion = \"title\"\ncontains = [\"x\"]",
        "only needs_input",
    ),
    (
        "state = \"working\"\nvisible = true\nregion = \"title\"\ncontains = [\"x\"]",
        "only idle",
    ),
    (
        "state = \"idle\"\nvisible = 1\nregion = \"title\"\ncontains = [\"x\"]",
        "visible takes",
    ),
    (
        "state = \"idle\"\npriority = 100001\nregion = \"title\"\ncontains = [\"x\"]",
        "priority must",
    ),
    (
        "state = \"idle\"\npriority = 0x10\nregion = \"title\"\ncontains = [\"x\"]",
        "priority must",
    ),
    (
        "state = \"idle\"\nregion = \"whole_recent\"\ncontains = [\"x\"]",
        "region must",
    ),
    ("state = \"idle\"\ncontains = [\"x\"]", "it has no region"),
    ("state = \"idle\"\nregion = \"title\"", "nothing to match"),
    (
        "state = \"idle\"\nregion = \"title\"\nline_regexp = ['x']",
        "unknown name \"line_regexp\"",
    ),
    (
        "state = \"idle\"\nregion = \"title\"\ncontains = [\"\"]",
        "each string must be",
    ),
    (
        "state = \"idle\"\nregion = \"title\"\ncontains = [1]",
        "each string must be",
    ),
    (
        "state = \"idle\"\nregion = \"title\"\nregex = ['(']",
        "does not compile",
    ),
    (
        "state = \"idle\"\nregion = \"title\"\nregex = ['(?:\\w{100}){100}']",
        "does not compile",
    ),
    (
        "state = \"idle\"\nregion = \"title\"\nany = [{}]",
        "is empty",
    ),
    (
        "state = \"idle\"\nregion = \"title\"\nany = { contains = [\"x\"] }",
        "a list of tables",
    ),
    (
        "state = \"idle\"\nregion = \"title\"\nnot = [{ name = \"x\" }]",
        "unknown name \"name\"",
    ),
    ("disable = true\nstate = \"idle\"", "only its name"),
    ("disable = false", "only true"),
];

#[test]
fn a_rule_with_anything_wrong_is_skipped_and_named_with_its_line() {
    let cases = WRONG;
    for (body, says) in cases {
        let parsed = one(body);
        assert!(
            parsed.rules.is_empty() && parsed.disabled.is_empty(),
            "{body}"
        );
        assert_eq!(parsed.problems.len(), 1, "{body}: {:?}", parsed.problems);
        let problem = &parsed.problems[0];
        assert!(problem.contains(says), "{body}: {problem}");
        assert!(problem.starts_with(char::is_numeric), "{problem}");
    }
}

#[test]
fn names_must_be_plain_present_and_unique() {
    let nameless = local(&format!("version = 1\n[[rule]]\n{GOOD}"));
    assert!(nameless.problems[0].contains("no name"));
    let odd = local(&format!("version = 1\n[[rule]]\nname = \"a b\"\n{GOOD}"));
    assert!(odd.problems[0].starts_with("3: a rule's name"));
    let twice = local(&format!(
        "version = 1\n[[rule]]\nname = \"a\"\n{GOOD}\n[[rule]]\nname = \"a\"\ndisable = true"
    ));
    assert_eq!(twice.rules.len(), 1);
    assert!(twice.disabled.is_empty());
    assert!(twice.problems[0].contains("rule \"a\" again"));
}

#[test]
fn only_the_local_file_disables() {
    let text = "version = 1\n[[rule]]\nname = \"a\"\ndisable = true";
    assert_eq!(local(text).disabled, ["a"]);
    let bundled = parse(text, Source::Bundled);
    assert!(bundled.disabled.is_empty());
    assert!(bundled.problems[0].contains("only the local file"));
}

#[test]
fn an_unknown_top_level_name_is_skipped_and_the_rest_applies() {
    let parsed = local(&format!(
        "version = 1\nagent = \"claude\"\n[[rule]]\nname = \"a\"\n{GOOD}"
    ));
    assert_eq!(parsed.rules.len(), 1);
    assert_eq!(parsed.problems, ["2: unknown name \"agent\", skipped"]);
}

#[test]
fn nesting_is_bounded() {
    let mut nested = String::from("contains = [\"x\"]");
    for _ in 0..MAX_DEPTH {
        nested = format!("any = [{{ {nested} }}]");
    }
    let deep_enough = one(&format!("state = \"idle\"\nregion = \"title\"\n{nested}"));
    assert_eq!(deep_enough.problems, Vec::<String>::new());
    let too_deep = one(&format!(
        "state = \"idle\"\nregion = \"title\"\nany = [{{ {nested} }}]"
    ));
    assert!(too_deep.problems[0].contains("nest at most"));
}

#[test]
fn matchers_per_rule_are_bounded() {
    let parts = |n: usize| {
        (0..n)
            .map(|i| format!("\"p{i}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let full = one(&format!(
        "state = \"idle\"\nregion = \"title\"\ncontains = [{}]",
        parts(MAX_MATCHERS)
    ));
    assert_eq!(full.rules.len(), 1);
    let over = one(&format!(
        "state = \"idle\"\nregion = \"title\"\ncontains = [{}]",
        parts(MAX_MATCHERS + 1)
    ));
    assert!(over.problems[0].contains("matchers"));
}

#[test]
fn patterns_per_file_and_rules_per_file_are_bounded() {
    let mut text = String::from("version = 1\n");
    for n in 0..=MAX_PATTERNS / 32 {
        let patterns = vec!["'a'"; 32].join(", ");
        let _ = write!(
            text,
            "[[rule]]\nname = \"p{n}\"\nstate = \"idle\"\nregion = \"title\"\nregex = [{patterns}]\n"
        );
    }
    let parsed = local(&text);
    assert_eq!(parsed.rules.len(), MAX_PATTERNS / 32);
    assert!(parsed.problems[0].contains("patterns in the file"));

    let mut text = String::from("version = 1\n");
    for n in 0..=MAX_RULES {
        let _ = write!(text, "[[rule]]\nname = \"r{n}\"\n{GOOD}\n");
    }
    let parsed = local(&text);
    assert_eq!(parsed.rules.len(), MAX_RULES);
    assert!(parsed.problems[0].contains("rules; the rest"));
}

#[test]
fn problems_are_capped_and_counted() {
    let mut text = String::from("version = 1\n");
    for n in 0..MAX_PROBLEMS + 5 {
        let _ = writeln!(text, "unknown{n} = 1");
    }
    let parsed = local(&text);
    assert_eq!(parsed.problems.len(), MAX_PROBLEMS);
    assert_eq!(parsed.more, 5);
}

#[test]
fn a_problem_quotes_a_long_name_cut_and_escaped() {
    let parsed = local(&format!("version = 1\n\"{}\\n\" = 1", "é".repeat(100)));
    let problem = &parsed.problems[0];
    assert!(problem.contains('…') && problem.contains("unknown name"));
    assert!(problem.len() < 200, "{problem}");
}

#[test]
fn a_problem_escapes_what_could_reorder_or_hide_its_text() {
    let parsed = parse("version = 1\n\"a\u{202E}b\u{200B}c\" = 1\n", Source::Local);
    let problem = &parsed.problems[0];
    assert!(problem.contains(r"\u{202e}"), "{problem}");
    assert!(problem.contains(r"\u{200b}"), "{problem}");
    assert!(!problem.contains('\u{202E}') && !problem.contains('\u{200B}'));
}

#[test]
fn a_recap_is_read_and_a_wrong_one_is_skipped_with_its_rule_kept() {
    let parsed = one(&format!(
        "{GOOD}\nrecap = '^(x+)'\nrecap_region = \"bottom(3)\""
    ));
    assert_eq!(parsed.problems, Vec::<String>::new());
    let recap = parsed.rules[0].recap.as_ref().unwrap();
    assert_eq!(recap.region.to_string(), "bottom(3)");
    let deadline = Deadline::after(Duration::from_secs(5));
    assert_eq!(
        recap.find("y\nxx z\nq", &deadline),
        Ok(Some("xx".to_owned()))
    );
    let own = one(&format!("{GOOD}\nrecap = 'z'"));
    assert_eq!(
        own.rules[0].recap.as_ref().unwrap().region.to_string(),
        "title"
    );
    for (body, says) in [
        ("recap = '('", "a pattern does not compile"),
        ("recap = ''", "recap must be a pattern of 1 to"),
        ("recap = 3", "recap must be a pattern of 1 to"),
        (
            "recap = 'a'\nrecap_region = \"bottom(0)\"",
            "recap_region must be one of",
        ),
        ("recap_region = \"title\"", "recap_region needs a recap"),
    ] {
        let parsed = one(&format!("{GOOD}\n{body}"));
        assert_eq!(parsed.rules.len(), 1, "{body}: the rule stays");
        assert!(parsed.rules[0].recap.is_none(), "{body}");
        assert_eq!(parsed.problems.len(), 1, "{body}");
        let problem = &parsed.problems[0];
        assert!(problem.contains(says), "{body}: {problem}");
        assert!(problem.ends_with("the recap is skipped"), "{problem}");
    }
}

#[test]
fn recaps_count_among_the_files_patterns() {
    let mut text = String::from("version = 1\n");
    // Three patterns a rule: the 86th rule's second is the file's 257th.
    for n in 0..100 {
        let _ = write!(
            text,
            "[[rule]]\nname = \"r{n}\"\nstate = \"working\"\nregion = \"title\"\nregex = ['a', 'a']\nrecap = 'b'\n"
        );
    }
    let parsed = local(&text);
    let with_recap = parsed
        .rules
        .iter()
        .filter(|rule| rule.recap.is_some())
        .count();
    assert_eq!(with_recap, MAX_PATTERNS / 3);
    assert!(parsed.problems[0].contains("patterns in the file"));
}
