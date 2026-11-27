//! The session tools' rules (ADR 0121, "The bridge and the channel"; step 6): each tool's
//! reach and whether it asks the person, the person's refusals, and what the person's
//! question says.
//!
//! Must not depend on: `app`, `dispatch`, threads or the disk; the app carries a
//! call out and asks these rules first.
//!
//! - A tool that asks starts nothing until the person's `y`, from the window, unless the
//!   person's configuration lets a session they started start sessions without asking, within
//!   [`starts_without_asking`]'s rules (ADR 0135). No tool types into a terminal, answers a
//!   session's dialog, or reads what a terminal withholds.
//! - The refusals and the rate are kept by session, never by connection: a session that
//!   connects again keeps them. No count of calls waits for the person (ADR 0177); a session
//!   makes at most [`guard::CALLS_A_MINUTE`] calls that ask or change in any minute (ADR 0178).
//! - Names, branches and paths in a question are the agent's or a repository's text: each is
//!   quoted, escaped and cut.

use oso_core::command::Reach;

mod add_repo;
pub mod answer;
mod archive;
mod guard;
mod rename;
pub mod repo;

pub use add_repo::{add_dir, add_repo_question, add_repo_said, addable};
pub use archive::{
    Archivable, Archive, Runs, archivable, archive_asks, archive_question, archived_said,
    registry_idle, stops_without_asking,
};
pub use guard::{Guard, Refusal};
pub use rename::{Renamable, renamable, rename_clash, rename_note, renamed_said};

use super::identity::Origin;

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
    /// Whether it starts a session, so that one started without asking counts toward the
    /// caller's `agents_start_limit` (ADR 0135); `space_add_repo` starts none (ADR 0161, Q4).
    pub starts: bool,
}

impl Tool {
    /// Whether it asks or changes.
    const fn asks_or_changes(&self) -> bool {
        matches!(self.reach, Reach::Change) || !matches!(self.asks, Asks::No)
    }

    /// The MCP hint that it only reads: it neither changes nor asks.
    pub const fn read_only(&self) -> bool {
        !self.asks_or_changes()
    }
}

const fn reads(name: &'static str) -> Tool {
    Tool {
        name,
        reach: Reach::Look,
        asks: Asks::No,
        destructive: false,
        outside: true,
        starts: false,
    }
}

/// The tools, the reading ones first. A change here is a change of what an agent may do: the
/// test below spells the table out, so that review sees it.
pub const TOOLS: [Tool; 14] = [
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
        starts: false,
    },
    Tool {
        name: "start_worktree_session",
        reach: Reach::Change,
        asks: Asks::UnlessAllowed,
        destructive: false,
        outside: false,
        starts: true,
    },
    Tool {
        name: "session_start",
        reach: Reach::Change,
        asks: Asks::UnlessAllowed,
        destructive: false,
        outside: false,
        starts: true,
    },
    // It writes only the person's message buffer of the caller's worktree, for oso's newest
    // request there, and commits nothing (ADR 0127, step 6a).
    Tool {
        name: "draft_set",
        reach: Reach::Change,
        asks: Asks::No,
        destructive: false,
        outside: false,
        starts: false,
    },
    // Only an outside session's, which moves into a space on the person's `y` (ADR 0151).
    Tool {
        name: "move_to_space",
        reach: Reach::Change,
        asks: Asks::Each,
        destructive: false,
        outside: true,
        starts: false,
    },
    // A space's orchestrator resumes a stopped session of its space, as `session-resume` does,
    // and as ADR 0135 lets a start go without asking (ADR 0153).
    Tool {
        name: "session_resume",
        reach: Reach::Change,
        asks: Asks::UnlessAllowed,
        destructive: false,
        outside: false,
        starts: true,
    },
    // A session's name in oso's record and its buffer's, no more: metadata only, as ADR 0121's
    // table has it (ADR 0155).
    Tool {
        name: "session_rename",
        reach: Reach::Change,
        asks: Asks::No,
        destructive: false,
        outside: false,
        starts: false,
    },
    // A space's orchestrator adds a repository to its space, as `space-add-repo` does; one oso
    // does not trust always asks (ADR 0161).
    Tool {
        name: "space_add_repo",
        reach: Reach::Change,
        asks: Asks::UnlessAllowed,
        destructive: false,
        outside: false,
        starts: false,
    },
    // A session's row, as the person's `A` archives it, which `A` undoes: a stopped one with
    // no question; one that runs only when idle, stopped first as ADR 0135 lets a start go
    // without asking (ADR 0179).
    Tool {
        name: "session_archive",
        reach: Reach::Change,
        asks: Asks::UnlessAllowed,
        destructive: false,
        outside: false,
        starts: false,
    },
    // A space's orchestrator's one answer to a native worker's tool call, as the person lets
    // it per tool: no question, as the call is the worker's, and the person sees it (ADR 0186).
    Tool {
        name: "session_answer",
        reach: Reach::Change,
        asks: Asks::No,
        destructive: false,
        outside: false,
        starts: false,
    },
];

/// The tool named `name`.
pub fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|tool| tool.name == name)
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

/// Whether a caller's calls may go without asking at all, where they otherwise would ask: the one
/// rule every such call is judged by first.
#[derive(Clone, Copy, Debug)]
pub struct CallerStanding {
    /// `[sessions] agents_start_without_asking` at oso's start, and no `agent-starts-ask` since.
    pub allowed: bool,
    /// How the calling session started.
    pub caller: Origin,
    /// It may be connected to claude.ai (ADR 0181): whoever holds the account may drive it, so each
    /// call of its that may go without asking otherwise asks the person at oso.
    pub connected: bool,
}

impl CallerStanding {
    /// Whether its calls may go without asking.
    pub fn goes_unasked(self) -> bool {
        self.allowed && self.caller.by_the_person() && !self.connected
    }
}

/// What a start without asking needs (ADR 0135).
#[derive(Clone, Copy, Debug)]
pub struct Unasked {
    /// The tool is one the setting covers: [`Asks::UnlessAllowed`].
    pub tool: Asks,
    /// The caller's standing.
    pub standing: CallerStanding,
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
    /// `"none"`: no limit.
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
        && unasked.standing.goes_unasked()
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

/// `said`, a question or what the echo area says of a start, with the model the new session
/// runs, which `is_model` took (ADR 0121, "A start's model, as amended"); as it was with none.
pub fn with_model(said: String, model: Option<&str>) -> String {
    match model {
        Some(model) => format!("{said} Model: {model}."),
        None => said,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod model_tests {
    /// The question, `?` and the echo of a start without asking name the model a tool asked
    /// for (ADR 0121, "A start's model, as amended"); none is said with none.
    #[test]
    fn a_question_names_the_model_the_new_session_runs() {
        let said = super::with_model("let it start?".to_owned(), None);
        assert_eq!(said, "let it start?");
        let said = super::with_model("let it start?".to_owned(), Some("claude-opus-5-5[1m]"));
        assert_eq!(said, "let it start? Model: claude-opus-5-5[1m].");
    }
}
