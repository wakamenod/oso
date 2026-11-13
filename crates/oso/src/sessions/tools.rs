//! The session tools' rules (ADR 0121, "The bridge and the channel"; step 6): each tool's
//! reach and whether it asks the person, the loop guard, the person's refusals, and what the
//! person's question says.
//!
//! Must not depend on: `app`, `dispatch`, `commands`, threads or the disk; the app carries a
//! call out and asks these rules first.
//!
//! - A tool that asks starts nothing until the person's `y`, from the window. No tool types
//!   into a terminal, answers a session's dialog, or reads what a terminal withholds.
//! - The loop guard and the refusals are kept by session, never by connection: a session that
//!   connects again keeps both.
//! - Names, branches and paths in a question are the agent's or a repository's text: each is
//!   quoted, escaped and cut.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use oso_core::command::Reach;

use super::identity::SessionId;

/// Whether a tool asks the person before it acts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asks {
    /// It acts, or only reads, with no question.
    No,
    /// Each call asks, as an `Agent:` question; `n` or `C-g` refuses it.
    Each,
}

/// A session tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tool {
    pub name: &'static str,
    pub reach: Reach,
    pub asks: Asks,
    /// The MCP hint that it may destroy what cannot be had back.
    pub destructive: bool,
    /// Whether a connection that is no hosted session's bridge may call it (the author's
    /// choice, step 6): only the reading tools, within the selected window's project.
    pub outside: bool,
}

impl Tool {
    /// Whether it counts toward the loop guard: every tool that asks or changes.
    pub const fn counted(&self) -> bool {
        matches!(self.reach, Reach::Change) || matches!(self.asks, Asks::Each)
    }

    /// The MCP hint that it only reads: it neither changes nor asks.
    pub const fn read_only(&self) -> bool {
        !self.counted()
    }
}

const fn reads(name: &'static str) -> Tool {
    Tool {
        name,
        reach: Reach::Look,
        asks: Asks::No,
        destructive: false,
        outside: true,
    }
}

/// The tools, the reading ones first. A change here is a change of what an agent may do: the
/// test below spells the table out, so that review sees it.
pub const TOOLS: [Tool; 6] = [
    reads("session_list"),
    reads("session_state"),
    // It runs git and `gh` with no question, as the author chose (ADR 0121, step 4).
    reads("worktree_list"),
    // No question once every check holds (Q14).
    Tool {
        name: "worktree_remove",
        reach: Reach::Change,
        asks: Asks::No,
        destructive: true,
        outside: false,
    },
    Tool {
        name: "start_worktree_session",
        reach: Reach::Change,
        asks: Asks::Each,
        destructive: false,
        outside: false,
    },
    Tool {
        name: "session_start",
        reach: Reach::Change,
        asks: Asks::Each,
        destructive: false,
        outside: false,
    },
];

/// The tool named `name`.
pub fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
}

/// The most calls that count a session may make since the person last typed in it.
pub const MAX_CALLS: u32 = 20;
/// How long the person's `n` refuses the same call from the same session without asking.
pub const REFUSED_FOR: Duration = Duration::from_secs(60);
/// The most refusals kept; the oldest goes first.
const MAX_REFUSALS: usize = 256;

/// Why the guard refuses a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The person refused the same call from this session less than [`REFUSED_FOR`] ago.
    Person,
    /// The session made [`MAX_CALLS`] since the person last typed in it.
    Loop,
}

impl Refusal {
    /// What the agent is told.
    pub fn why(self) -> &'static str {
        match self {
            Self::Person => {
                "the person refused this a moment ago; it is their decision: do not try it another way"
            }
            Self::Loop => "waits for the person to type in this session",
        }
    }
}

/// The loop guard and the person's refusals, by session.
#[derive(Debug, Default)]
pub struct Guard {
    calls: HashMap<SessionId, u32>,
    refused: HashMap<(SessionId, &'static str, String), Instant>,
}

impl Guard {
    /// Counts a call of `tool` on `target` from `session` at `now`, or says why it is refused.
    /// A call this refuses is not counted; one it lets through is, whatever stops it later.
    pub fn admit(
        &mut self,
        session: &SessionId,
        tool: &'static str,
        target: &str,
        now: Instant,
    ) -> Result<(), Refusal> {
        let key = (session.clone(), tool, target.to_owned());
        match self.refused.get(&key) {
            Some(&at) if now.saturating_duration_since(at) < REFUSED_FOR => {
                return Err(Refusal::Person);
            }
            Some(_) => {
                self.refused.remove(&key);
            }
            None => {}
        }
        let calls = self.calls.entry(session.clone()).or_default();
        if *calls >= MAX_CALLS {
            return Err(Refusal::Loop);
        }
        *calls += 1;
        Ok(())
    }

    /// Whether the person refused `tool` on `target` from `session` less than
    /// [`REFUSED_FOR`] before `now`: for a target only known once a job has looked, such as
    /// the branch git resolved a name to. It counts nothing.
    pub fn was_refused(
        &self,
        session: &SessionId,
        tool: &'static str,
        target: &str,
        now: Instant,
    ) -> bool {
        self.refused
            .get(&(session.clone(), tool, target.to_owned()))
            .is_some_and(|&at| now.saturating_duration_since(at) < REFUSED_FOR)
    }

    /// The person's input reached `session`'s terminal: it may make [`MAX_CALLS`] again.
    pub fn person_typed(&mut self, session: &SessionId) {
        self.calls.remove(session);
    }

    /// The person refused `tool` on `target` from `session` at `now`.
    pub fn refused(&mut self, session: &SessionId, tool: &'static str, target: &str, now: Instant) {
        if self.refused.len() >= MAX_REFUSALS {
            self.refused
                .retain(|_, &mut at| now.saturating_duration_since(at) < REFUSED_FOR);
        }
        if self.refused.len() >= MAX_REFUSALS
            && let Some(oldest) = self
                .refused
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(key, _)| key.clone())
        {
            self.refused.remove(&oldest);
        }
        self.refused
            .insert((session.clone(), tool, target.to_owned()), now);
    }

    /// The calls `session` has made since the person last typed in it.
    #[cfg(test)]
    pub fn calls(&self, session: &SessionId) -> u32 {
        self.calls.get(session).copied().unwrap_or(0)
    }
}

/// The longest a quoted field of a question is shown, in characters.
const FIELD: usize = 80;

/// `text` quoted for a question: its quotes and backslashes escaped, and every character
/// that is not printable ASCII written as a `\u{…}` escape, so that no invisible, look-alike,
/// control or bidirectional character can change how the question reads. Past [`FIELD`]
/// characters its middle goes, so that a path keeps the end that names it.
pub fn quoted(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' | '\\' => {
                escaped.push('\\');
                escaped.push(c);
            }
            ' '..='~' => escaped.push(c),
            other => escaped.push_str(&format!("\\u{{{:x}}}", u32::from(other))),
        }
    }
    let count = escaped.chars().count();
    if count > FIELD {
        let head: String = escaped.chars().take(FIELD / 4).collect();
        let tail: String = escaped
            .chars()
            .skip(count - (FIELD - FIELD / 4 - 1))
            .collect();
        escaped = format!("{head}…{tail}");
    }
    format!("\"{escaped}\"")
}

/// A brief's size in lines, for a question.
pub fn lines(brief: &str) -> usize {
    brief.lines().count().max(1)
}

fn brief_part(lines: usize) -> String {
    let unit = if lines == 1 { "line" } else { "lines" };
    format!(", with a brief of {lines} {unit} (? to read it)")
}

/// Where a worktree session's branch starts, for its question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch<'a> {
    /// A branch that exists, at its tip.
    Existing,
    /// A new branch, from this revision, as shown.
    New { from: &'a str },
}

/// The question of `start_worktree_session`: `caller` starts a session in a new worktree at
/// `place` on `branch`.
pub fn worktree_question(
    caller: &str,
    place: &str,
    branch: &str,
    start: Branch<'_>,
    brief_lines: usize,
) -> String {
    let on = match start {
        Branch::Existing => format!("on existing branch {}", quoted(branch)),
        Branch::New { from } => format!("on new branch {} from {}", quoted(branch), quoted(from)),
    };
    format!(
        "let session {} start a session in a new worktree {} {on}{}?",
        quoted(caller),
        quoted(place),
        brief_part(brief_lines)
    )
}

/// The question of `session_start`: `caller` starts a session in `directory`, with a brief of
/// `brief_lines` if it has one; `shared` when another session runs there.
pub fn start_question(
    caller: &str,
    directory: &str,
    brief_lines: Option<usize>,
    shared: bool,
) -> String {
    let brief = brief_lines.map(brief_part).unwrap_or_default();
    let shared = if shared {
        " Another session runs there."
    } else {
        ""
    };
    format!(
        "let session {} start a session in {}{brief}?{shared}",
        quoted(caller),
        quoted(directory)
    )
}

#[cfg(test)]
mod tests;
