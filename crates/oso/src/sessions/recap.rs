//! A session's recap (ADR 0121, step 3): one line of what it is about, for `*Sessions*` and the
//! switch float. Needs input: the line the matched rule's `recap` pattern finds, the dialog's
//! question. Working: the line a working rule's `recap` finds, the TUI's status line. Otherwise
//! the title.
//!
//! Must not depend on: `app`, `commands` or `dispatch`; plain functions over the evidence, run
//! on the sessions worker next to the rules and never on the UI thread.
//!
//! The text is the screen's, so a model wrote it: it is cut to [`MAX_CHARS`], with control and
//! bidirectional formatting characters dropped, before it leaves the worker. No model is
//! asked, and nothing is written anywhere.

use std::collections::HashSet;

use regex::Regex;

use super::engine::Verdict;
use super::rules::{Deadline, Late, RuleSet};
use super::screen::{Evidence, Region};
use super::state::Target;

/// The most characters of a recap; a longer one ends in `…`.
pub const MAX_CHARS: usize = 80;

/// A rule's `recap`: a pattern matched against each line of a region, from the bottom up.
#[derive(Debug)]
pub struct Recap {
    pattern: Regex,
    pub region: Region,
}

impl Recap {
    pub fn new(pattern: Regex, region: Region) -> Self {
        Self { pattern, region }
    }

    /// What the pattern finds in `text`, the region's, on the lowest line it matches: its
    /// first capture if it has one that took part, else the whole match. Lines whose find is
    /// empty once cleaned are passed over. [`Late`] if `deadline` passed first.
    pub fn find(&self, text: &str, deadline: &Deadline) -> Result<Option<String>, Late> {
        for line in text.lines().rev() {
            deadline.check()?;
            let Some(captures) = self.pattern.captures(line) else {
                continue;
            };
            let Some(found) = captures.get(1).or_else(|| captures.get(0)) else {
                continue;
            };
            let found = clean(found.as_str());
            if !found.is_empty() {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }
}

/// The recap of `evidence`, which `verdict` was given on, or `None` to keep the last one: the
/// verdict holds, or the deadline passed. The rules in `dropped` are passed over.
pub fn recap(
    rules: &RuleSet,
    verdict: &Verdict,
    evidence: &Evidence,
    dropped: &HashSet<String>,
    deadline: &Deadline,
) -> Option<String> {
    let found = match (verdict.target, &verdict.by) {
        (Target::Hold, _) => return None,
        (Target::NeedsInput(_), Ok(matched)) => {
            let rule = rules.rules().iter().find(|rule| rule.name == matched.name);
            match rule.and_then(|rule| rule.recap.as_ref()) {
                Some(recap) => recap.find(&evidence.region(recap.region), deadline).ok()?,
                None => None,
            }
        }
        // The rule that matched first, then any other working rule with a recap: the footer
        // tells a turn is under way, and the status line says what it does.
        (Target::Working, Ok(matched)) => {
            let first = rules
                .rules()
                .iter()
                .filter(|rule| rule.name == matched.name);
            let others = rules.rules().iter().filter(|rule| {
                rule.target == Target::Working
                    && rule.name != matched.name
                    && !dropped.contains(&rule.name)
            });
            let mut found = None;
            for recap in first.chain(others).filter_map(|rule| rule.recap.as_ref()) {
                found = recap.find(&evidence.region(recap.region), deadline).ok()?;
                if found.is_some() {
                    break;
                }
            }
            found
        }
        _ => None,
    };
    Some(found.unwrap_or_else(|| clean(&evidence.region(Region::Title))))
}

/// `text` on one line: control and bidirectional formatting characters dropped, runs of
/// white space made one space, trimmed, and cut to [`MAX_CHARS`] with `…`.
pub fn clean(text: &str) -> String {
    let words = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .map(|word| word.chars().filter(|&c| !is_bidi_format(c)))
        .filter(|word| word.clone().next().is_some());
    let mut out = String::new();
    let mut chars = 0;
    for word in words {
        for c in (chars > 0).then_some(' ').into_iter().chain(word) {
            if chars == MAX_CHARS {
                out.pop();
                out.push('…');
                return out;
            }
            out.push(c);
            chars += 1;
        }
    }
    out
}

/// Whether `c` reorders text or marks its direction: a recap must not change how what is
/// drawn beside it reads.
fn is_bidi_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests;
