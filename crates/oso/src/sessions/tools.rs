//! The session tools' rules (ADR 0121, "The bridge and the channel"; step 6): each tool's
//! reach and whether it asks the person, the loop guard, the person's refusals, and what the
//! person's question says.
//!
//! Must not depend on: `app`, `dispatch`, threads or the disk; the app carries a
//! call out and asks these rules first.
//!
//! - A tool that asks starts nothing until the person's `y`, from the window, unless the
//!   person's configuration lets a session they started start sessions without asking, within
//!   [`starts_without_asking`]'s rules (ADR 0135). No tool types into a terminal, answers a
//!   session's dialog, or reads what a terminal withholds.
//! - The loop guard and the refusals are kept by session, never by connection: a session that
//!   connects again keeps both.
//! - Names, branches and paths in a question are the agent's or a repository's text: each is
//!   quoted, escaped and cut.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use oso_core::command::Reach;

mod rename;
pub mod repo;

pub use rename::{Renamable, renamable, rename_clash, rename_note, renamed_said};

use super::identity::Origin;
use super::identity::SessionId;

/// Whether a tool asks the person before it acts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asks {
    /// It acts, or only reads, with no question.
    No,
    /// Each call asks, as an `Agent:` question; `n` or `C-g` refuses it.
    Each,
    /// As [`Asks::Each`], unless the person's configuration lets the caller start sessions
    /// without asking, within [`starts_without_asking`]'s rules (ADR 0135).
    UnlessAllowed,
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
    /// choice, step 6): the reading tools, within the selected window's project, and
    /// `move_to_space`, which asks each time (ADR 0151).
    pub outside: bool,
}

impl Tool {
    /// Whether it counts toward the loop guard: every tool that asks or changes.
    pub const fn counted(&self) -> bool {
        matches!(self.reach, Reach::Change) || !matches!(self.asks, Asks::No)
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
pub const TOOLS: [Tool; 11] = [
    reads("session_list"),
    reads("session_state"),
    // It runs git and `gh` with no question, as the author chose (ADR 0121, step 4).
    reads("worktree_list"),
    // A hosted session's own space, which another connection has none of (ADR 0136, step 4).
    Tool {
        outside: false,
        ..reads("space_status")
    },
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
        asks: Asks::UnlessAllowed,
        destructive: false,
        outside: false,
    },
    Tool {
        name: "session_start",
        reach: Reach::Change,
        asks: Asks::UnlessAllowed,
        destructive: false,
        outside: false,
    },
    // It writes only the person's message buffer of the caller's worktree, for oso's newest
    // request there, and commits nothing (ADR 0127, step 6a).
    Tool {
        name: "draft_set",
        reach: Reach::Change,
        asks: Asks::No,
        destructive: false,
        outside: false,
    },
    // Only an outside session's, which moves into a space on the person's `y` (ADR 0151).
    Tool {
        name: "move_to_space",
        reach: Reach::Change,
        asks: Asks::Each,
        destructive: false,
        outside: true,
    },
    // A space's orchestrator resumes a stopped session of its space, as `session-resume` does,
    // and as ADR 0135 lets a start go without asking (ADR 0153).
    Tool {
        name: "session_resume",
        reach: Reach::Change,
        asks: Asks::UnlessAllowed,
        destructive: false,
        outside: false,
    },
    // A session's name in oso's record and its buffer's, no more: metadata only, as ADR 0121's
    // table has it (ADR 0155).
    Tool {
        name: "session_rename",
        reach: Reach::Change,
        asks: Asks::No,
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
    quoted_within(text, FIELD)
}

/// `text` quoted as [`quoted`] quotes it, its middle going only past `field` characters.
pub fn quoted_within(text: &str, field: usize) -> String {
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
    if count > field {
        let head: String = escaped.chars().take(field / 4).collect();
        let tail: String = escaped
            .chars()
            .skip(count - (field - field / 4 - 1))
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
    format!(
        "let session {} start {}{}?",
        quoted(caller),
        in_worktree(place, branch, start),
        brief_part(brief_lines)
    )
}

/// What the echo area says of a `start_worktree_session` that started with no question
/// (ADR 0135), with `explain` the keys of `session-explain` (ADR 0143).
pub fn worktree_started(
    caller: &str,
    place: &str,
    branch: &str,
    start: Branch<'_>,
    brief_lines: usize,
    explain: &str,
) -> String {
    format!(
        "session {} is starting {}, without asking ({}; {explain} explains)",
        quoted(caller),
        in_worktree(place, branch, start),
        brief_said(Some(brief_lines))
    )
}

/// "a session in a new worktree … on … branch …".
fn in_worktree(place: &str, branch: &str, start: Branch<'_>) -> String {
    let on = match start {
        Branch::Existing => format!("on existing branch {}", quoted(branch)),
        Branch::New { from } => format!("on new branch {} from {}", quoted(branch), quoted(from)),
    };
    format!("a session in a new worktree {} {on}", quoted(place))
}

/// "brief: N lines", or "no brief".
fn brief_said(lines: Option<usize>) -> String {
    match lines {
        Some(1) => "brief: 1 line".to_owned(),
        Some(lines) => format!("brief: {lines} lines"),
        None => "no brief".to_owned(),
    }
}

/// What a start without asking needs (ADR 0135).
#[derive(Clone, Copy, Debug)]
pub struct Unasked {
    /// The tool is one the setting covers: [`Asks::UnlessAllowed`].
    pub tool: Asks,
    /// `[sessions] agents_start_without_asking` at oso's start, and no `agent-starts-ask` since.
    pub allowed: bool,
    /// How the calling session started.
    pub caller: Origin,
    /// The project the session would start in is trusted, its directory's repository the
    /// caller's.
    pub trusted: bool,
    /// The caller's live sessions that started without asking, and those starting.
    pub live_unasked: usize,
    /// `[sessions] agents_start_limit`.
    pub limit: StartLimit,
}

/// `[sessions] agents_start_limit`: how many live sessions one caller may have started without
/// asking (ADR 0135, Q5, and ADR 0144).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartLimit {
    /// At most this many.
    Most(u8),
    /// `"none"`: as many as the loop guard lets the caller start.
    Unlimited,
}

impl StartLimit {
    /// Whether a caller with `live` such sessions may start one more.
    pub fn allows(self, live: usize) -> bool {
        match self {
            Self::Most(most) => live < usize::from(most),
            Self::Unlimited => true,
        }
    }
}

/// Whether a call that would ask starts without asking: only as the person's configuration
/// allows, from a session the person started, so that it reaches one level, in a trusted
/// project, and below the caller's limit, if it has one; else it asks as before (ADR 0135).
pub fn starts_without_asking(unasked: Unasked) -> bool {
    unasked.tool == Asks::UnlessAllowed
        && unasked.allowed
        && unasked.caller.by_the_person()
        && unasked.trusted
        && unasked.limit.allows(unasked.live_unasked)
}

/// What the echo area says of a `session_start` that started with no question (ADR 0135), with
/// `explain` the keys of `session-explain` (ADR 0143).
pub fn start_started(
    caller: &str,
    directory: &str,
    brief_lines: Option<usize>,
    explain: &str,
) -> String {
    format!(
        "session {} is starting a session in {}, without asking ({}; {explain} explains)",
        quoted(caller),
        quoted(directory),
        brief_said(brief_lines)
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

/// The question of `session_resume`: `caller` resumes session `name` in `directory`.
pub fn resume_question(caller: &str, name: &str, directory: &str) -> String {
    format!(
        "let session {} resume session {} in {}?",
        quoted(caller),
        quoted(name),
        quoted(directory)
    )
}

/// What the echo area says of a `session_resume` that went ahead with no question (ADR 0153).
pub fn resume_started(caller: &str, name: &str, directory: &str) -> String {
    format!(
        "session {} is resuming session {} in {}, without asking",
        quoted(caller),
        quoted(name),
        quoted(directory)
    )
}

/// What `session_resume` knows of the session it would resume (ADR 0153).
#[derive(Clone, Copy, Debug, Default)]
pub struct Resumable {
    /// It is the calling session.
    pub caller: bool,
    /// Its `claude` runs, or is being started.
    pub runs: bool,
    /// The person archived it.
    pub archived: bool,
    /// It is an orchestrator of the space, current or earlier: the person's to resume.
    pub orchestrator: bool,
    /// Its terminal buffer is there, which the resume runs in again.
    pub buffer: bool,
}

/// Whether an orchestrator's `session_resume` may resume a session of its space, or why not:
/// only a stopped or failed one, not archived, that is no orchestrator and still has its
/// buffer, and never the caller (ADR 0153).
///
/// # Errors
///
/// Why it is refused, for the agent.
pub fn resumable(session: Resumable) -> Result<(), &'static str> {
    if session.caller {
        return Err("that is your own session");
    }
    if session.runs {
        return Err("that session runs already");
    }
    if session.archived {
        return Err("that session is archived; only the person resumes it");
    }
    if session.orchestrator {
        return Err("that is an orchestrator of your space; only the person resumes it");
    }
    if !session.buffer {
        return Err("that session's buffer is gone; only the person resumes it");
    }
    Ok(())
}

/// `said`, a question or what the echo area says of a start, with the directories the new
/// session also reads (ADR 0136, step 4b), each quoted; as it was with none.
pub fn also_reads(said: String, dirs: &[std::path::PathBuf]) -> String {
    if dirs.is_empty() {
        return said;
    }
    let dirs: Vec<String> = (dirs.iter())
        .map(|dir| quoted(&dir.display().to_string()))
        .collect();
    let count = match dirs.len() {
        1 => "1 more directory".to_owned(),
        n => format!("{n} more directories"),
    };
    format!("{said} It also reads {count}: {}.", dirs.join(", "))
}

#[cfg(test)]
mod tests;
