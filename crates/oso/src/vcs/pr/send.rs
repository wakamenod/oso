//! The message a pull request's checks or review threads make for an agent session (ADR 0127,
//! step 6b): what GitHub said, under the untrusted line, each line quoted or fenced, which the
//! person reads in `*review message*` before it is pasted.
//!
//! Must not depend on: `gh`, git, the file system or `app`; pure functions of what `gh` said.
//!
//! - A check gives its name, workflow, state, link and description; a failed GitHub Actions
//!   job's log tail, read by the thread, goes in a fence one backtick longer than any run in it.
//! - A review thread gives its file and line, the diff hunk in a `diff` fence and its comments,
//!   each quoted with `> `. Only the comments of the person, the login `gh` is logged in as,
//!   and of the repository's owners, members and collaborators go in (Q13, amended in step 6b:
//!   the pull request's author only as one of those). The others are counted, and kept apart
//!   in the same shape for `C-c C-t` to take in.
//! - Every line is escaped as a review's hunk lines are, so that a paste makes none of them
//!   several, and the message is bounded: what goes past it is counted.

use super::issue::{END, GHOST, UNTRUSTED};
use super::{Bucket, Check, Comment, GhRepo, Thread, text};
use crate::git::printable;
use crate::review::message::{backticks, line_text};

/// The fields of `gh pr view --json` a message is made from.
pub(crate) const SEND_FIELDS: &str =
    "number,url,author,baseRefName,headRefName,headRefOid,isCrossRepository";
/// The most failing checks whose logs one message reads (Q5).
pub(crate) const MAX_LOGS: usize = 8;
/// The most lines of a log's tail.
const LOG_LINES: usize = 200;
/// The most bytes of a log's tail.
const LOG_BYTES: usize = 32 * 1024;
/// The most bytes of what is left out, for `C-c C-t`.
const MAX_OTHERS: usize = 64 * 1024;
/// The most lines of a thread's hunk sent: its last ones, where the comment is.
const HUNK_LINES: usize = 30;
/// What the message asks of the session, after what GitHub said.
const ASK: &str = "Fix these, then tell me; do not push.\n";

/// What a message says of the pull request itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SendPull {
    pub(crate) number: u64,
    pub(crate) url: String,
    pub(crate) author: String,
    pub(crate) base: String,
    pub(crate) branch: String,
    pub(crate) head_commit: String,
    /// Its head is in another repository, a fork's: no worktree of the person's holds it.
    pub(crate) cross: bool,
}

/// The pull request in `gh pr view --json` [`SEND_FIELDS`]'s answer.
pub(crate) fn read_send(json: &[u8]) -> Option<SendPull> {
    let value = serde_json::from_slice::<serde_json::Value>(json).ok()?;
    let login = |field: &str| (value.get(field)?.get("login")).and_then(text);
    Some(SendPull {
        number: value.get("number")?.as_u64()?,
        url: value.get("url").and_then(text).unwrap_or_default(),
        author: login("author").unwrap_or_default(),
        base: value.get("baseRefName").and_then(text).unwrap_or_default(),
        branch: text(value.get("headRefName")?)?,
        head_commit: text(value.get("headRefOid")?)?,
        cross: value
            .get("isCrossRepository")
            .and_then(serde_json::Value::as_bool)
            == Some(true),
    })
}

/// What a message is made of, as the key at a line picked it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    /// The check of this link, or of this name for one with no link (`x`).
    Check { name: String, link: String },
    /// Every failing check (`X`).
    Failing,
    /// The threads at this file and line (`c`).
    Thread { path: String, line: Option<u64> },
    /// Every unresolved thread (`C`).
    Unresolved,
}

impl Pick {
    /// The checks it takes of `checks`.
    pub(crate) fn checks<'a>(&self, checks: &'a [Check]) -> Vec<&'a Check> {
        match self {
            Self::Check { name, link } => (checks.iter())
                .filter(|c| {
                    if link.is_empty() {
                        c.name == *name
                    } else {
                        c.link == *link
                    }
                })
                .take(1)
                .collect(),
            Self::Failing => checks.iter().filter(|c| c.bucket == Bucket::Fail).collect(),
            Self::Thread { .. } | Self::Unresolved => Vec::new(),
        }
    }

    /// The threads it takes of `threads`.
    pub(crate) fn threads<'a>(&self, threads: &'a [Thread]) -> Vec<&'a Thread> {
        match self {
            Self::Thread { path, line } => (threads.iter())
                .filter(|t| t.path == *path && t.line == *line)
                .collect(),
            Self::Unresolved => threads.iter().filter(|t| !t.resolved).collect(),
            Self::Check { .. } | Self::Failing => Vec::new(),
        }
    }

    /// Whether it is of threads, so the threads are read.
    pub(crate) fn of_threads(&self) -> bool {
        matches!(self, Self::Thread { .. } | Self::Unresolved)
    }
}

/// A check's log, as the thread read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Log {
    /// None is read: it did not fail, or is not a GitHub Actions job of the repository.
    None,
    /// Its failed steps' tail, made by [`log_tail`].
    Tail(String),
    /// Why it was not read.
    Unread(String),
}

/// The message made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrMessage {
    pub(crate) text: String,
    /// The comments left out, in the message's shape, for `C-c C-t` to take in; empty if none.
    pub(crate) others: String,
    /// How many comments were left out.
    pub(crate) left_out: usize,
    /// How many checks and threads it holds, and how many went past its bound.
    pub(crate) items: usize,
    pub(crate) cut: usize,
}

/// The message of `checks`, each with its log, and of `threads`, of pull request `pull` of
/// `repo`, `person` being the login `gh` is logged in as, at most `limit` bytes.
pub(crate) fn compose(
    repo: &GhRepo,
    pull: &SendPull,
    checks: &[(&Check, Log)],
    threads: &[&Thread],
    person: &str,
    limit: usize,
) -> PrMessage {
    let mut entries: Vec<String> = Vec::new();
    for (check, log) in checks {
        entries.push(check_entry(check, log));
    }
    let mut others = String::new();
    let mut left_out = 0;
    for thread in threads {
        let (kept, out): (Vec<&Comment>, Vec<&Comment>) =
            (thread.comments.iter()).partition(|c| trusted(c, person));
        left_out += out.len();
        if !out.is_empty() && others.len() < MAX_OTHERS {
            others.push_str(&thread_entry(thread, &out));
        }
        if !kept.is_empty() {
            entries.push(thread_entry(thread, &kept));
        }
    }
    let mut text = format!("{UNTRUSTED}\n\n");
    text.push_str(&format!(
        "Pull request #{} on GitHub, {}: {} into {}.\n",
        pull.number,
        printable(repo.as_str(), 200),
        printable(&pull.branch, 200),
        printable(&pull.base, 200),
    ));
    let note = |left_out: usize, cut: usize| {
        let mut note = String::new();
        if left_out > 0 {
            note.push_str(&format!(
                "\n{left_out} comments by others than you and the repository's owners, members \
                 and collaborators are left out.\n"
            ));
        }
        if cut > 0 {
            note.push_str(&format!(
                "\n{cut} more were cut: past the message's size.\n"
            ));
        }
        note
    };
    let room = |cut| limit.saturating_sub(note(left_out, cut).len() + END.len() + ASK.len() + 64);
    let total = entries.len();
    let mut items = 0;
    for entry in entries {
        if text.len() + entry.len() > room(total - items) {
            break;
        }
        text.push_str(&entry);
        items += 1;
    }
    let cut = total - items;
    text.push_str(&note(left_out, cut));
    text.push_str(END);
    text.push('\n');
    text.push_str(ASK);
    if !others.is_empty() {
        others.insert_str(0, "\nComments by others, which you took in, quoted:\n");
    }
    PrMessage {
        text,
        others,
        left_out,
        items,
        cut,
    }
}

/// Whether `comment` goes in: the person's, or of the repository's owners, members and
/// collaborators. A deleted account's, or an empty login, is no one's.
fn trusted(comment: &Comment, person: &str) -> bool {
    let known = |login: &str| !login.is_empty() && login != GHOST;
    (known(person) && comment.author == person)
        || matches!(
            comment.association.as_str(),
            "OWNER" | "MEMBER" | "COLLABORATOR"
        )
}

fn check_entry(check: &Check, log: &Log) -> String {
    let state = match check.bucket {
        Bucket::Pass => "passed",
        Bucket::Fail => "failed",
        Bucket::Pending => "pending",
        Bucket::Skipping => "skipped",
        Bucket::Cancel => "cancelled",
    };
    let mut out = format!("\n## Check {}", printable(&check.name, 200));
    if !check.workflow.is_empty() {
        out.push_str(&format!(" ({})", printable(&check.workflow, 200)));
    }
    out.push_str(&format!(": {state}\n"));
    if !check.link.is_empty() {
        out.push_str(&format!("Link: {}\n", printable(&check.link, 500)));
    }
    if !check.description.is_empty() {
        out.push_str("Description, quoted:\n");
        push_quoted(&mut out, &check.description);
    }
    match log {
        Log::None => {}
        Log::Unread(why) => out.push_str(&format!(
            "(Its log was not read: {}.)\n",
            printable(why, 300)
        )),
        Log::Tail(tail) => {
            out.push_str("The end of its failed steps' log:\n");
            push_fenced(&mut out, "text", tail);
        }
    }
    out
}

fn thread_entry(thread: &Thread, comments: &[&Comment]) -> String {
    let mut out = format!("\n## {}", printable(&thread.path, 300));
    if let Some(line) = thread.line {
        out.push_str(&format!("  L{line}"));
    }
    if thread.resolved {
        out.push_str("  (resolved)");
    }
    out.push('\n');
    if let Some(hunk) = (thread.comments.first())
        .map(|c| c.hunk.as_str())
        .filter(|h| !h.is_empty())
    {
        let lines: Vec<&str> = hunk.lines().collect();
        let from = lines.len().saturating_sub(HUNK_LINES);
        if from > 0 {
            out.push_str(&format!("(The hunk's first {from} lines are left out.)\n"));
        }
        push_fenced(&mut out, "diff", &lines[from..].join("\n"));
    }
    for comment in comments {
        out.push_str(&format!(
            "Comment by @{} ({}), quoted:\n",
            printable(&comment.author, 40),
            printable(&comment.association, 20)
        ));
        push_quoted(&mut out, &comment.body);
    }
    if thread.more > 0 {
        out.push_str(&format!(
            "({} more comments of this thread were not read.)\n",
            thread.more
        ));
    }
    out
}

/// `text`'s lines, each escaped and quoted with `> `.
fn push_quoted(out: &mut String, text: &str) {
    for line in text.lines() {
        out.push_str("> ");
        out.push_str(&line_text(line.as_bytes()));
        out.push('\n');
    }
}

/// `text`'s lines, each escaped, in a fence of `kind` one backtick longer than any run in them.
fn push_fenced(out: &mut String, kind: &str, text: &str) {
    let lines: Vec<String> = text
        .lines()
        .map(|line| line_text(line.as_bytes()))
        .collect();
    let longest = lines.iter().map(|line| backticks(line)).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    out.push_str(&format!("{fence}{kind}\n"));
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&fence);
    out.push('\n');
}

/// The run and job of a check's `link`, if it is a GitHub Actions job of `repo` itself:
/// `https://HOST/OWNER/NAME/actions/runs/RUN/job/JOB`, nothing else.
pub(crate) fn job_of(link: &str, repo: &GhRepo) -> Option<(u64, u64)> {
    let rest = link.strip_prefix("https://")?;
    let (host, owner, name) = repo.parts();
    let mut parts = rest.split('/');
    let mut next = || parts.next();
    let same = |a: Option<&str>, b: &str| a.is_some_and(|a| a.eq_ignore_ascii_case(b));
    if !(same(next(), host) && same(next(), owner) && same(next(), name)) {
        return None;
    }
    let number = |part: Option<&str>| {
        part.filter(|p| !p.is_empty() && p.len() <= 20 && p.bytes().all(|b| b.is_ascii_digit()))?
            .parse::<u64>()
            .ok()
    };
    (next() == Some("actions") && next() == Some("runs")).then_some(())?;
    let run = number(next())?;
    (next() == Some("job")).then_some(())?;
    let job = number(next())?;
    next().is_none().then_some((run, job))
}

/// The tail of a job's failed log, `gh run view --log-failed`'s answer: its escape sequences
/// taken out, its last [`LOG_LINES`] lines, within [`LOG_BYTES`].
pub(crate) fn log_tail(out: &[u8]) -> String {
    let text = strip_escapes(&String::from_utf8_lossy(out));
    let lines: Vec<&str> = text.lines().collect();
    let from = lines.len().saturating_sub(LOG_LINES);
    let mut tail = lines[from..].join("\n");
    if tail.len() > LOG_BYTES {
        let mut at = tail.len() - LOG_BYTES;
        while !tail.is_char_boundary(at) {
            at += 1;
        }
        tail.drain(..at);
    }
    tail
}

/// `text` without its terminal escape sequences: CSI (`ESC [ … final`), OSC (`ESC ] …` ended by
/// BEL or `ESC \`) and two-character ones.
fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
#[path = "send/tests.rs"]
mod tests;
