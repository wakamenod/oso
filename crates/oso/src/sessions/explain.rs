//! What `*Session explain*` shows (ADR 0121, Q4): the verdict, the rule that gave it and its
//! source, the region text it matched, or why no rule did, so that the person can write a rule
//! in the local file the same day the TUI changes.
//!
//! Must not depend on: `app`, `commands` or `dispatch`; text in, text out.
//!
//! The text holds the session's screen, which is untrusted and may be secret, so the buffer is
//! the person's only: the channel cannot read it. Its size is bounded by the evidence's, about
//! 41 KiB of rows and a few lines more.

use std::fmt::Write as _;

use super::engine::Verdict;
use super::rules::RuleSet;
use super::screen::Evidence;
use super::state::Target;

/// The buffer's name.
pub const BUFFER: &str = "*Session explain*";

/// The explanation of `verdict`, which `rules` gave on `evidence`, of the screen called `what`.
pub fn text(what: &str, rules: &RuleSet, evidence: &Evidence, verdict: &Verdict) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Session explain: {what}\n");
    let _ = writeln!(out, "Verdict: {}", says(verdict.target));
    match &verdict.by {
        Ok(matched) => {
            let _ = writeln!(
                out,
                "Rule: {} ({}), priority {}",
                matched.name,
                matched.source.name(),
                matched.priority
            );
            let _ = writeln!(out, "Region: {}", matched.region);
            out.push_str("Matched text:\n");
            quote(&mut out, matched.text.lines());
        }
        Err(fallback) => {
            out.push_str("Rule: none\n");
            let _ = writeln!(out, "Why: {}", fallback.says());
            let _ = writeln!(out, "Screen, its last {} rows:", evidence.rows().len());
            quote(&mut out, evidence.rows().iter().map(String::as_str));
        }
    }
    let _ = writeln!(
        out,
        "\nRules in force: {} bundled, {} local, {} bundled disabled",
        rules.bundled, rules.local, rules.disabled
    );
    out.push_str(
        "No state authorises anything: not a paste, not an answer, not an action of an agent.\n",
    );
    out
}

fn says(target: Target) -> String {
    match target {
        Target::NeedsInput(reason) => format!("needs input ({})", reason.name()),
        Target::Working => "working".to_owned(),
        Target::Idle { visible: true } => "idle, a visible sign".to_owned(),
        Target::Idle { visible: false } => {
            "idle, once confirmed: 3 looks 100 ms apart, at most 700 ms".to_owned()
        }
        Target::Hold => "hold: the state keeps its last value".to_owned(),
    }
}

fn quote<'l>(out: &mut String, lines: impl Iterator<Item = &'l str>) {
    for line in lines {
        let _ = writeln!(out, "  │ {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::text;
    use crate::sessions::engine::evaluate;
    use crate::sessions::rules::RuleSet;
    use crate::sessions::screen::Evidence;

    #[test]
    fn a_match_names_its_rule_source_and_text() {
        let rules = RuleSet::bundled();
        let evidence = Evidence::from_text(include_str!("screens/question.txt"));
        let shown = text(
            "question.txt",
            &rules,
            &evidence,
            &evaluate(&rules, &evidence),
        );
        assert!(shown.starts_with("Session explain: question.txt\n"));
        assert!(shown.contains("Verdict: needs input (question)"));
        assert!(shown.contains("Rule: question (bundled), priority 920"));
        assert!(shown.contains("Region: after_last_rule"));
        assert!(shown.contains("  │   4. Chat about this"));
        assert!(shown.contains("bundled, 0 local, 0 bundled disabled"));
    }

    #[test]
    fn no_match_shows_why_and_the_screen() {
        let rules = RuleSet::bundled();
        let evidence = Evidence::from_text("Proceed?\n> ok");
        let shown = text("x", &rules, &evidence, &evaluate(&rules, &evidence));
        assert!(shown.contains("Rule: none\nWhy: no rule matched"));
        assert!(shown.contains("never as needs input"));
        assert!(shown.contains("Screen, its last 2 rows:\n  │ Proceed?\n  │ > ok\n"));
    }
}
