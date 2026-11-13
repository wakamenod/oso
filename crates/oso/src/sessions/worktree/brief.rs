//! A worktree session's first prompt (ADR 0121, "Worktree sessions"): a header that says where
//! the session is, the line that tells it whom to report back to, the hand-off facts the
//! starting session leaves behind, and the person's brief, together at most [`MAX_BRIEF`]
//! bytes, as one argument. Facts that do not fit are dropped, least useful first.
//!
//! Must not depend on: the file system, `app` or `dispatch`; it is a plain function
//! over what the worker found, and shows untrusted text as `porcelain::printable` does.

use std::fmt;
use std::path::{Path, PathBuf};

use super::porcelain::printable;

/// The most bytes the whole first prompt takes.
pub const MAX_BRIEF: usize = 64 * 1024;

/// The most characters shown of one fact or one part of the header.
const MAX_PART_CHARS: usize = 4096;

/// The line above the facts.
const FACTS: &str = "Hand-off facts from that session:";

/// Where the new session is.
#[derive(Debug, Clone, Copy)]
pub struct Header<'a> {
    /// The repository's main checkout.
    pub repo: &'a Path,
    /// The new worktree.
    pub path: &'a Path,
    /// The branch checked out there.
    pub branch: &'a str,
    /// What the branch started from.
    pub base: &'a str,
}

/// What the starting session leaves behind; every field may be empty.
#[derive(Debug, Clone, Copy, Default)]
pub struct Handoff<'a> {
    /// Its name, only when it is live and named.
    pub name: Option<&'a str>,
    /// Its transcript.
    pub transcript: Option<&'a Path>,
    /// Its plan files.
    pub plans: &'a [PathBuf],
    /// `git status --porcelain` lines of its directory.
    pub uncommitted: &'a [String],
    /// The files it touched recently.
    pub touched: &'a [PathBuf],
}

/// Why a brief cannot be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BriefError {
    /// Nothing but whitespace.
    Empty,
    /// It holds a NUL byte, which no argument can.
    Nul,
    /// The header, the report-back line and the brief alone are over [`MAX_BRIEF`].
    TooLong,
}

impl fmt::Display for BriefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "The brief is empty",
            Self::Nul => "The brief holds a NUL byte",
            Self::TooLong => "The brief is over 64 KiB",
        })
    }
}

impl std::error::Error for BriefError {}

/// A list of facts under a heading.
struct Section {
    heading: &'static str,
    entries: Vec<String>,
    /// How many of `entries` are shown.
    kept: usize,
    /// The bytes `entries[..kept]` take, each as its own indented line.
    bytes: usize,
}

impl Section {
    fn new(heading: &'static str, entries: Vec<String>) -> Self {
        let bytes = entries.iter().map(|entry| entry_len(entry)).sum();
        Self {
            heading,
            kept: entries.len(),
            entries,
            bytes,
        }
    }

    /// The bytes it adds, each line after a line break; nothing once no entry is shown.
    fn len(&self) -> usize {
        if self.kept == 0 {
            return 0;
        }
        let more = self.more().map_or(0, |more| 1 + more.len());
        1 + self.heading.len() + self.bytes + more
    }

    /// The line that says how many entries were dropped, if any were.
    fn more(&self) -> Option<String> {
        let dropped = self.entries.len() - self.kept;
        (dropped > 0).then(|| format!("  … and {dropped} more"))
    }

    /// Drops the last entry shown; false if none is left.
    fn drop_last(&mut self) -> bool {
        let Some(last) = self.kept.checked_sub(1) else {
            return false;
        };
        self.bytes -= entry_len(&self.entries[last]);
        self.kept = last;
        true
    }

    fn write(&self, out: &mut String) {
        if self.kept == 0 {
            return;
        }
        out.push('\n');
        out.push_str(self.heading);
        for entry in &self.entries[..self.kept] {
            out.push_str("\n  ");
            out.push_str(entry);
        }
        if let Some(more) = self.more() {
            out.push('\n');
            out.push_str(&more);
        }
    }
}

/// The bytes an entry takes as a line: a line break, two spaces and itself.
fn entry_len(entry: &str) -> usize {
    3 + entry.len()
}

/// The hand-off facts, as far as they fit.
struct Facts {
    transcript: Option<String>,
    /// Plans, uncommitted changes and touched files, in the order shown.
    sections: [Section; 3],
}

impl Facts {
    fn new(handoff: &Handoff<'_>) -> Self {
        let paths = |paths: &[PathBuf]| paths.iter().map(|p| shown_path(p)).collect();
        Self {
            transcript: handoff
                .transcript
                .map(|path| format!("- Its transcript: {}", shown_path(path))),
            sections: [
                Section::new("- Its plan files:", paths(handoff.plans)),
                Section::new(
                    "- Its uncommitted changes, which did not come along:",
                    handoff.uncommitted.iter().map(|line| clean(line)).collect(),
                ),
                Section::new("- Files it touched recently:", paths(handoff.touched)),
            ],
        }
    }

    /// The bytes the facts add after the header, with the blank line and [`FACTS`].
    fn len(&self) -> usize {
        let body = self.transcript.as_ref().map_or(0, |line| 1 + line.len())
            + self.sections.iter().map(Section::len).sum::<usize>();
        if body == 0 { 0 } else { 2 + FACTS.len() + body }
    }

    /// Drops one fact: touched files from the end, then uncommitted lines, then plans, then the
    /// transcript. False if none is left.
    fn drop_one(&mut self) -> bool {
        for section in self.sections.iter_mut().rev() {
            if section.drop_last() {
                return true;
            }
        }
        self.transcript.take().is_some()
    }

    fn write(&self, out: &mut String) {
        if self.len() == 0 {
            return;
        }
        out.push_str("\n\n");
        out.push_str(FACTS);
        if let Some(line) = &self.transcript {
            out.push('\n');
            out.push_str(line);
        }
        for section in &self.sections {
            section.write(out);
        }
    }
}

/// The text passed as the session's first prompt: the header, the report-back line when the
/// starting session is live and named, the hand-off facts that fit, and `brief` without its
/// trailing whitespace.
///
/// # Errors
///
/// [`BriefError::Empty`] for a brief of only whitespace, [`BriefError::Nul`] for one with a NUL,
/// and [`BriefError::TooLong`] when no fact need be shown and it is still over [`MAX_BRIEF`].
pub fn compose(
    header: &Header<'_>,
    handoff: Option<&Handoff<'_>>,
    brief: &str,
) -> Result<String, BriefError> {
    if brief.trim().is_empty() {
        return Err(BriefError::Empty);
    }
    if brief.contains('\0') {
        return Err(BriefError::Nul);
    }
    let brief = brief.trim_end();
    let mut head = format!(
        "You are in a git worktree of {} at {}, on branch {} from {}.",
        shown_path(header.repo),
        shown_path(header.path),
        clean(header.branch),
        clean(header.base),
    );
    if let Some(name) = handoff.and_then(|handoff| handoff.name) {
        let name = clean(name);
        head.push_str(&format!(
            "\nThis work was handed to you by session {name}; report back with SendMessage (to: {name})."
        ));
    }
    let fixed = head.len() + 2 + brief.len();
    if fixed > MAX_BRIEF {
        return Err(BriefError::TooLong);
    }
    let mut facts = handoff.map(Facts::new);
    if let Some(facts) = facts.as_mut() {
        while fixed + facts.len() > MAX_BRIEF && facts.drop_one() {}
    }
    let mut out = head;
    if let Some(facts) = &facts {
        facts.write(&mut out);
    }
    out.push_str("\n\n");
    out.push_str(brief);
    Ok(out)
}

/// A path as text for the prompt.
fn shown_path(path: &Path) -> String {
    clean(&path.to_string_lossy())
}

/// `text` with control characters, line and paragraph separators and bidirectional controls as
/// `\u{…}` escapes, at most [`MAX_PART_CHARS`] characters of it, then `…`.
fn clean(text: &str) -> String {
    printable(text, MAX_PART_CHARS)
}

#[cfg(test)]
mod tests;
