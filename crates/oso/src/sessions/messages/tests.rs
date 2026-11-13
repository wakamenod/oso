//! Message rules: how a `[[message]]` table is read, laid over the bundled ones, and how a
//! look finds the senders of lines it did not show before.
//!
//! Must not depend on: `app` or `dispatch`; these tests drive the sessions' own
//! parts, with no window and no terminal.

use std::time::Duration;

use super::{MAX_FROM_BYTES, MAX_SENDERS, Seen, senders};
use crate::sessions::rules::{Deadline, Late, Parsed, RuleSet, Source, parse};
use crate::sessions::screen::{Evidence, Region};

const RULE: &str = r#"
version = 1

[[message]]
name = "message"
pattern = '^\s*› Message from @(?P<from>[^:\s]+):'
"#;

fn rules(text: &str) -> RuleSet {
    let local = parse(text, Source::Local);
    assert!(local.problems.is_empty(), "{:?}", local.problems);
    RuleSet::merged(Parsed::default(), local)
}

fn look(rules: &RuleSet, seen: &mut Seen, screen: &str) -> Vec<String> {
    let evidence = Evidence::new(screen.lines(), "", None);
    let deadline = Deadline::after(Duration::from_secs(1));
    senders(rules.messages(), &evidence, seen, &deadline).unwrap()
}

#[test]
fn a_new_message_line_names_its_sender_once() {
    let rules = rules(RULE);
    let mut seen = Seen::default();
    let screen = "● Done.\n› Message from @api-fix: tests pass\n> ";
    assert_eq!(look(&rules, &mut seen, screen), ["api-fix"]);
    assert!(look(&rules, &mut seen, screen).is_empty(), "the same look");
    // Scrolled up a row, the line is still the one seen.
    let scrolled = "› Message from @api-fix: tests pass\n> \nnew output";
    assert!(look(&rules, &mut seen, scrolled).is_empty());
}

#[test]
fn the_same_line_again_is_a_new_message() {
    let rules = rules(RULE);
    let mut seen = Seen::default();
    let once = "› Message from @docs: done\n> ";
    assert_eq!(look(&rules, &mut seen, once), ["docs"]);
    let twice = "› Message from @docs: done\nok\n› Message from @docs: done\n> ";
    assert_eq!(look(&rules, &mut seen, twice), ["docs"]);
}

#[test]
fn senders_come_in_screen_order_each_once() {
    let rules = rules(RULE);
    let mut seen = Seen::default();
    let screen = "› Message from @b: x\n› Message from @a: y\n› Message from @b: z\n";
    assert_eq!(look(&rules, &mut seen, screen), ["b", "a"]);
    let many: String = (0..20)
        .map(|n| format!("› Message from @s{n}: hi\n"))
        .collect();
    assert_eq!(look(&rules, &mut seen, &many).len(), MAX_SENDERS);
}

#[test]
fn a_sender_is_cleaned_and_cut() {
    let rules = rules(RULE);
    let mut seen = Seen::default();
    let long = "é".repeat(MAX_FROM_BYTES);
    let screen = format!("› Message from @a\u{202e}b: x\n› Message from @{long}: y\n");
    let found = look(&rules, &mut seen, &screen);
    assert_eq!(found[0], "ab");
    assert_eq!(found[1].len(), MAX_FROM_BYTES);
    assert!(long.starts_with(&found[1]));
}

#[test]
fn past_the_deadline_nothing_is_found_and_the_lines_wait() {
    let rules = rules(RULE);
    let mut seen = Seen::default();
    let screen = "› Message from @api: x\n";
    let evidence = Evidence::new(screen.lines(), "", None);
    let late = Deadline::after(Duration::ZERO);
    assert_eq!(
        senders(rules.messages(), &evidence, &mut seen, &late),
        Err(Late)
    );
    assert_eq!(look(&rules, &mut seen, screen), ["api"]);
}

#[test]
fn a_message_rule_needs_a_pattern_with_a_from_capture() {
    let cases = [
        ("pattern = 'Message from @(\\S+)'", "no capture (?P<from>…)"),
        ("region = 'bottom(3)'", "it has no pattern"),
        (
            "pattern = 'x(?P<from>y)'\nstate = 'idle'",
            "unknown name \"state\"",
        ),
        (
            "pattern = 'x(?P<from>y)'\nregion = 'nowhere'",
            "region must be",
        ),
        ("pattern = '('", "does not compile"),
    ];
    for (body, said) in cases {
        let text = format!("version = 1\n[[message]]\nname = \"m\"\n{body}\n");
        let parsed = parse(&text, Source::Local);
        assert!(parsed.messages.is_empty(), "{body}");
        assert!(
            parsed.problems.iter().any(|problem| problem.contains(said)),
            "{body}: {:?}",
            parsed.problems
        );
    }
}

#[test]
fn a_message_rule_reads_the_last_40_rows_unless_it_names_a_region() {
    let parsed = parse(RULE, Source::Local);
    assert_eq!(parsed.messages[0].region, Region::Bottom(40));
    let text = format!("{RULE}region = 'after_last_rule'\n");
    let parsed = parse(&text, Source::Local);
    assert_eq!(parsed.messages[0].region, Region::AfterLastRule);
}

#[test]
fn the_local_file_replaces_and_disables_bundled_message_rules() {
    let bundled = parse(RULE, Source::Bundled);
    let replaced = RuleSet::merged(
        parse(RULE, Source::Bundled),
        parse(
            "version = 1\n[[message]]\nname = \"message\"\npattern = 'Msg (?P<from>\\w+)'\n",
            Source::Local,
        ),
    );
    assert_eq!(replaced.messages().len(), 1);
    assert_eq!(
        replaced.messages()[0].pattern.as_str(),
        "Msg (?P<from>\\w+)"
    );
    let disabled = RuleSet::merged(
        bundled,
        parse(
            "version = 1\n[[message]]\nname = \"message\"\ndisable = true\n",
            Source::Local,
        ),
    );
    assert!(disabled.messages().is_empty());
    // A name a state rule has is taken.
    let text = format!(
        "{RULE}\n[[rule]]\nname = \"message\"\nstate = \"idle\"\nregion = \"title\"\ncontains = [\"x\"]\n"
    );
    let parsed = parse(&text, Source::Local);
    assert!(
        parsed.problems.iter().any(|p| p.contains("again")),
        "{:?}",
        parsed.problems
    );
}

#[test]
fn the_bundled_message_rule_finds_the_sender() {
    let rules = RuleSet::bundled();
    assert!(!rules.messages().is_empty());
    let mut seen = Seen::default();
    let screen = include_str!("../screens/message.txt");
    assert_eq!(look(&rules, &mut seen, screen), ["sender"]);
}
