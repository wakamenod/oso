//! The rules over one look at a session's screen (ADR 0121): the first rule, highest priority
//! first, whose test holds over its region gives the verdict, and what matched is kept for
//! `session-explain`.
//!
//! Must not depend on: `app`, `commands` or `dispatch`; plain functions over text, run on the
//! sessions worker and never on the UI thread.
//!
//! Each region's text is built once per evaluation, with its lower-case copy for `contains`.
//! The work is linear in the region's text for each pattern, and both are bounded
//! (`screen::MAX_ROWS`, `rules::MAX_PATTERNS`).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::rules::{BUDGET, Deadline, Late, RuleSet, Source};
use super::screen::{Evidence, Region};
use super::state::Target;

/// Why no rule's verdict was taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// No rule matched: idle. An unknown prompt is idle too, never needs input.
    NoRule,
    /// Echo is off, so the rows are withheld: the state keeps its last value.
    EchoOff,
    /// The rules took longer than [`BUDGET`]: the state keeps its last value. `slowest` is the
    /// rule that took longest on this look.
    TooSlow { slowest: String },
}

impl Fallback {
    pub fn says(&self) -> String {
        match self {
            Self::NoRule => {
                "no rule matched: idle. An unknown prompt counts as idle, never as needs input"
                    .to_owned()
            }
            Self::EchoOff => {
                "echo off: the rows are withheld, and the state keeps its last value".to_owned()
            }
            Self::TooSlow { slowest } => format!(
                "rules took too long, over {} ms: the state keeps its last value. The slowest \
                 rule was {slowest}",
                BUDGET.as_millis()
            ),
        }
    }
}

/// The rule that matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matched {
    pub name: String,
    pub source: Source,
    pub priority: i64,
    pub region: Region,
    /// The region's text it matched.
    pub text: String,
}

/// What the rules said of one look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub target: Target,
    /// The rule whose verdict it is, or why there is none.
    pub by: Result<Matched, Fallback>,
}

/// The verdict of `rules` on `evidence`, within [`BUDGET`].
pub fn evaluate(rules: &RuleSet, evidence: &Evidence) -> Verdict {
    evaluate_within(rules, evidence, BUDGET, &HashSet::new())
}

/// The verdict of `rules` on `evidence`, or hold if they take longer than `budget`. The rules
/// named in `dropped` are passed over: the worker stopped using them for running late.
pub fn evaluate_within(
    rules: &RuleSet,
    evidence: &Evidence,
    budget: Duration,
    dropped: &HashSet<String>,
) -> Verdict {
    if evidence.is_echo_off() {
        return Verdict {
            target: Target::Hold,
            by: Err(Fallback::EchoOff),
        };
    }
    let deadline = Deadline::after(budget);
    let mut slowest: Option<(Duration, &str)> = None;
    let mut texts: Vec<(Region, String, String)> = Vec::new();
    for rule in rules.rules() {
        if dropped.contains(&rule.name) {
            continue;
        }
        let at = match texts.iter().position(|(region, ..)| *region == rule.region) {
            Some(at) => at,
            None => {
                let text = evidence.region(rule.region);
                let lower = text.to_ascii_lowercase();
                texts.push((rule.region, text, lower));
                texts.len() - 1
            }
        };
        let (_, text, lower) = &texts[at];
        let started = Instant::now();
        let held = rule.holds(text, lower, &deadline);
        let took = started.elapsed();
        if slowest.is_none_or(|(most, _)| took > most) {
            slowest = Some((took, &rule.name));
        }
        match held {
            Ok(true) => {
                return Verdict {
                    target: rule.target,
                    by: Ok(Matched {
                        name: rule.name.clone(),
                        source: rule.source,
                        priority: rule.priority,
                        region: rule.region,
                        text: text.clone(),
                    }),
                };
            }
            Ok(false) => {}
            Err(Late) => {
                let slowest = slowest
                    .map_or(rule.name.as_str(), |(_, name)| name)
                    .to_owned();
                return Verdict {
                    target: Target::Hold,
                    by: Err(Fallback::TooSlow { slowest }),
                };
            }
        }
    }
    Verdict {
        target: Target::Idle { visible: false },
        by: Err(Fallback::NoRule),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use super::{Fallback, Target, evaluate, evaluate_within};
    use crate::sessions::rules::{Parsed, RuleSet, Source, parse};
    use crate::sessions::screen::{Evidence, Region};
    use crate::sessions::state::Reason;

    fn set(bundled: &str, local: &str) -> RuleSet {
        let bundled = parse(bundled, Source::Bundled);
        assert!(bundled.problems.is_empty(), "{:?}", bundled.problems);
        let local = if local.is_empty() {
            Parsed::default()
        } else {
            parse(local, Source::Local)
        };
        assert!(local.problems.is_empty(), "{:?}", local.problems);
        RuleSet::merged(bundled, local)
    }

    const RULES: &str = r#"
version = 1

[[rule]]
name = "prompt"
state = "idle"
visible = true
priority = 10
region = "prompt_box"
line_regex = ['^❯(\s|$)']

[[rule]]
name = "busy"
state = "working"
priority = 20
region = "after_last_rule"
contains = ["ESC to interrupt"]

[[rule]]
name = "ask"
state = "needs_input"
reason = "question"
priority = 30
region = "bottom(5)"
contains = ["esc to cancel"]
any = [{ contains = ["enter to select"] }, { regex = ['(?i)enter to confirm'] }]
not = [{ contains = ["help"] }]
"#;

    fn screen(footer: &str) -> Evidence {
        Evidence::from_text(&format!("out\n─────\n❯ \n─────\n{footer}\n"))
    }

    #[test]
    fn the_highest_priority_rule_that_holds_wins_and_says_what_it_matched() {
        let rules = set(RULES, "");
        let verdict = evaluate(&rules, &screen("? for shortcuts"));
        assert_eq!(verdict.target, Target::Idle { visible: true });
        let matched = verdict.by.unwrap();
        assert_eq!(matched.name, "prompt");
        assert_eq!(matched.region, Region::PromptBox);
        assert_eq!(matched.text, "❯");
        assert_eq!(
            evaluate(&rules, &screen("esc to interrupt")).target,
            Target::Working,
            "contains ignores ASCII case"
        );
        let asked = evaluate(&rules, &screen("Enter to select · Esc to cancel"));
        assert_eq!(asked.target, Target::NeedsInput(Reason::Question));
        assert_eq!(
            evaluate(&rules, &screen("Enter to confirm · Esc to cancel")).target,
            Target::NeedsInput(Reason::Question)
        );
    }

    #[test]
    fn not_and_any_gate_a_rule() {
        let rules = set(RULES, "");
        let help = evaluate(&rules, &screen("Help · Enter to select · Esc to cancel"));
        assert_eq!(help.by.unwrap().name, "prompt", "not keeps the dialog out");
        let neither = evaluate(&rules, &screen("Esc to cancel"));
        assert_eq!(neither.by.unwrap().name, "prompt", "any needs one");
    }

    #[test]
    fn no_rule_is_idle_and_echo_off_holds() {
        let rules = set(RULES, "");
        let none = evaluate(
            &rules,
            &Evidence::from_text("Do you want to proceed?\n1. Yes"),
        );
        assert_eq!(none.target, Target::Idle { visible: false });
        assert_eq!(none.by, Err(Fallback::NoRule));
        let off = evaluate(&rules, &Evidence::echo_off("title", None));
        assert_eq!(off.target, Target::Hold);
        assert_eq!(off.by, Err(Fallback::EchoOff));
    }

    #[test]
    fn a_local_rule_replaces_or_disables_a_bundled_one_by_name() {
        let local = r#"
version = 1
[[rule]]
name = "busy"
state = "working"
priority = 20
region = "title"
regex = ['^[⠁-⣿]']

[[rule]]
name = "prompt"
disable = true
"#;
        let rules = set(RULES, local);
        assert_eq!((rules.bundled, rules.local, rules.disabled), (1, 1, 1));
        let shown = screen("esc to interrupt");
        assert_eq!(evaluate(&rules, &shown).by, Err(Fallback::NoRule));
        let spinning = Evidence::new(["x"].into_iter(), "⠋ Claude", None);
        let verdict = evaluate(&rules, &spinning);
        assert_eq!(verdict.target, Target::Working);
        assert_eq!(verdict.by.unwrap().source, Source::Local);
    }

    #[test]
    fn rules_past_their_budget_hold_and_name_the_slowest() {
        let rules = set(RULES, "");
        let none = HashSet::new();
        let verdict = evaluate_within(&rules, &screen("? for shortcuts"), Duration::ZERO, &none);
        assert_eq!(verdict.target, Target::Hold);
        let Err(Fallback::TooSlow { slowest }) = &verdict.by else {
            panic!("{verdict:?}");
        };
        assert_eq!(slowest, "ask", "the first rule tried, the only one");
        assert!(
            verdict
                .by
                .as_ref()
                .unwrap_err()
                .says()
                .contains("over 5 ms")
        );
        let in_time = evaluate_within(
            &rules,
            &screen("? for shortcuts"),
            Duration::from_secs(5),
            &none,
        );
        assert_eq!(in_time.target, Target::Idle { visible: true });
        let dropped = HashSet::from(["ask".to_owned()]);
        let passed_over =
            evaluate_within(&rules, &screen("? for shortcuts"), Duration::ZERO, &dropped);
        assert_ne!(passed_over.by, verdict.by, "the dropped rule is not tried");
    }

    #[test]
    fn line_regex_matches_lines_and_regex_the_whole_region() {
        let rules = set(
            r#"
version = 1
[[rule]]
name = "two"
state = "working"
region = "bottom(3)"
line_regex = ['^b$', '^c$']
regex = ['(?s)a.*c']
"#,
            "",
        );
        let verdict = evaluate(&rules, &Evidence::from_text("a\nb\nc"));
        assert_eq!(verdict.target, Target::Working);
        let verdict = evaluate(&rules, &Evidence::from_text("a\nbc\n"));
        assert_eq!(verdict.by, Err(Fallback::NoRule));
    }
}
