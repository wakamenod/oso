//! Messages between sessions on a session's screen (ADR 0121, Q18, step 5): a `[[message]]`
//! rule finds the line the TUI shows when another session's `SendMessage` arrives, and the
//! sender's name in it, so that the session's row says who wrote and `session-follow-message`
//! shows that session.
//!
//! Must not depend on: `app`, `dispatch` or `config`; rows in, names out.
//!
//! The line is screen text, which any program in the terminal could print: a sender found is
//! a pointer to a session for the person, never proof of who wrote, and it authorises nothing.
//! Only lines that were not on the last look count, so a message already seen and scrolled is
//! not found again. The name is cleaned as a recap is and kept to [`MAX_FROM_BYTES`].

use regex::Regex;

use super::recap::clean;
use super::rules::{Deadline, Late};
use super::screen::{Evidence, Region};

/// The most senders one look gives.
pub const MAX_SENDERS: usize = 8;

/// The longest sender's name kept, in bytes.
pub const MAX_FROM_BYTES: usize = 64;

/// The name of the capture that holds the sender.
pub const FROM: &str = "from";

/// A rule that finds a message's line and its sender, from a `[[message]]` table.
#[derive(Debug)]
pub struct MessageRule {
    pub name: String,
    /// The rows it reads, line by line.
    pub region: Region,
    /// A line's pattern, with a capture named [`FROM`].
    pub pattern: Regex,
}

/// The message lines of the newest look, to tell new ones from those seen before.
#[derive(Debug, Default)]
pub struct Seen {
    lines: Vec<String>,
}

/// The senders of the message lines `evidence` shows that `seen` did not hold, in screen
/// order, each once, at most [`MAX_SENDERS`]; `seen` then holds this look's lines. A line
/// shown twice counts once more than it was. Past `deadline`, nothing is found and `seen`
/// is kept, so the next look finds them.
pub fn senders(
    rules: &[MessageRule],
    evidence: &Evidence,
    seen: &mut Seen,
    deadline: &Deadline,
) -> Result<Vec<String>, Late> {
    let mut lines = Vec::new();
    let mut found: Vec<String> = Vec::new();
    let mut before = seen.lines.clone();
    for rule in rules {
        deadline.check()?;
        let text = evidence.region(rule.region);
        for line in text.lines() {
            let Some(from) = rule
                .pattern
                .captures(line)
                .and_then(|captures| captures.name(FROM))
            else {
                continue;
            };
            lines.push(line.to_owned());
            match before.iter().position(|old| old == line) {
                Some(at) => {
                    before.swap_remove(at);
                }
                None => {
                    let from = cut(&clean(from.as_str()));
                    if !from.is_empty() && !found.contains(&from) && found.len() < MAX_SENDERS {
                        found.push(from);
                    }
                }
            }
        }
    }
    seen.lines = lines;
    Ok(found)
}

/// `name` cut to [`MAX_FROM_BYTES`] at a character's end.
fn cut(name: &str) -> String {
    let mut end = name.len().min(MAX_FROM_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}

#[cfg(test)]
mod tests;
