//! A native session's earlier turns (ADR 0165, step 5): which lines of its transcript are
//! shown, and what the rows say of those that are not.
//!
//! Must not depend on: files, threads or the UI; the caller reads the lines and gives each
//! with its offset.
//!
//! - The lines are placed in the conversation by their ids. What is shown is the chain from
//!   the newest line that shows anything back through each line's parent, so that a branch
//!   left by a rewind is not, and on across a compaction.
//! - The newest whole turns are shown, within [`MAX_BYTES`] of their lines and [`MAX_LINES`]
//!   lines: a cut starts at a message of the person's, and the rows say how many of the
//!   person's messages are before it.
//! - At most [`MAX_INDEXED`] lines are placed, many times the longest transcript measured; a
//!   line's parent is found only among the lines before it, so the chain always goes back in
//!   the file, and ends. A chain whose oldest line names a parent not placed, as one past the
//!   part of the file read or one that did not read, is cut there, and the rows say so.
//! - Whatever is shown is said to be as the transcript records it: a file the person's own
//!   tools can write.
//! - `Debug` gives counts only: what a conversation says is never logged.

use std::collections::HashMap;
use std::fmt;

use super::wire::transcript::{self, Said, Shows};

/// The most bytes of the lines shown, in all; the newest line is shown whatever its length.
pub const MAX_BYTES: u64 = 8 << 20;
/// The most lines shown.
pub const MAX_LINES: usize = 2000;
/// The most lines placed: four times the 32,000 of the longest transcript measured.
pub const MAX_INDEXED: usize = 1 << 17;

/// A line placed in the conversation.
#[derive(Debug)]
struct Entry {
    /// The entry of the line it follows.
    parent: Option<u32>,
    /// It names a parent that is not placed.
    orphan: bool,
    shows: Shows,
    at: u64,
    len: u32,
}

/// The lines of a transcript placed in the conversation, in the file's order.
#[derive(Default)]
pub struct Index {
    entries: Vec<Entry>,
    by_id: HashMap<Box<str>, u32>,
    /// Lines of the conversation that did not read.
    unreadable: usize,
}

impl fmt::Debug for Index {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Index")
            .field("entries", &self.entries.len())
            .field("unreadable", &self.unreadable)
            .finish()
    }
}

/// A line to read and show: its offset, its length and what it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub at: u64,
    pub len: usize,
    pub shows: Shows,
}

/// The lines shown, oldest first, and what is left out before them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Chosen {
    pub lines: Vec<Shown>,
    /// Earlier lines of the conversation are not shown.
    pub cut: bool,
    /// How many of them are messages of the person's.
    pub prompts_left_out: usize,
}

impl Index {
    /// Places `line`, at offset `at` of its file. Whether more lines may be placed.
    pub fn take(&mut self, at: u64, line: &[u8]) -> bool {
        if self.entries.len() >= MAX_INDEXED {
            return false;
        }
        let head = match transcript::head(line) {
            Ok(Some(head)) => head,
            Ok(None) => return true,
            Err(transcript::Unreadable) => {
                self.unreadable += 1;
                return true;
            }
        };
        let (Ok(index), Ok(len)) = (u32::try_from(self.entries.len()), u32::try_from(line.len()))
        else {
            return false;
        };
        let parent = (head.parent.as_deref()).and_then(|parent| self.by_id.get(parent).copied());
        self.entries.push(Entry {
            parent,
            orphan: head.parent.is_some() && parent.is_none(),
            shows: head.shows,
            at,
            len,
        });
        self.by_id.insert(head.uuid.into_boxed_str(), index);
        true
    }

    /// Whether no line is placed.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many lines of the conversation did not read.
    pub fn unreadable(&self) -> usize {
        self.unreadable
    }

    /// The lines shown of the chain that ends at the newest line showing anything.
    pub fn chosen(&self) -> Chosen {
        let shows = |entry: &Entry| entry.shows != Shows::Nothing;
        let Some(newest) = self.entries.iter().rposition(shows) else {
            return Chosen::default();
        };
        // Newest first; each parent is before its line, so this ends.
        let mut chain = Vec::new();
        let mut at = Some(newest);
        // Its oldest line follows one not placed: what was before is not known.
        let mut broken = false;
        while let Some(index) = at {
            let entry = &self.entries[index];
            if shows(entry) {
                chain.push(entry);
            }
            broken = entry.orphan;
            at = entry.parent.map(|parent| parent as usize);
        }
        let mut bytes = 0u64;
        let mut kept = 0;
        for entry in &chain {
            let len = u64::from(entry.len);
            if kept > 0 && (kept == MAX_LINES || bytes + len > MAX_BYTES) {
                break;
            }
            bytes += len;
            kept += 1;
        }
        let mut window = &chain[..kept];
        let cut = kept < chain.len();
        // A cut starts at a message of the person's, where there is one.
        if cut && let Some(prompt) = window.iter().rposition(|e| e.shows == Shows::Prompt) {
            window = &window[..=prompt];
        }
        let prompts_left_out = (chain[window.len()..].iter())
            .filter(|entry| entry.shows == Shows::Prompt)
            .count();
        Chosen {
            lines: (window.iter().rev())
                .map(|entry| Shown {
                    at: entry.at,
                    len: entry.len as usize,
                    shows: entry.shows,
                })
                .collect(),
            cut: broken || window.len() < chain.len(),
            prompts_left_out,
        }
    }
}

/// Why a session's earlier turns are not shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// No transcript of its conversation was found.
    NotFound,
    /// Its transcript is not a regular file of this user's with one name, or did not read.
    NotRead,
    /// Its transcript says the conversation ran in another directory.
    Elsewhere,
    /// Reading its transcript took longer than the start gives it.
    TooSlow,
    /// Its transcript has more lines than are placed.
    TooMany,
}

/// A session's earlier turns, as its transcript says them, oldest first.
#[derive(Default, PartialEq)]
pub struct Past {
    pub said: Vec<Said>,
    /// Earlier lines of the conversation are not shown, and how many of them are messages of
    /// the person's.
    pub cut: bool,
    pub prompts_left_out: usize,
    /// Lines of the transcript that did not read, or were too long to.
    pub unreadable: usize,
    pub missing: Option<Missing>,
}

impl fmt::Debug for Past {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Past")
            .field("said", &self.said.len())
            .field("cut", &self.cut)
            .field("prompts_left_out", &self.prompts_left_out)
            .field("unreadable", &self.unreadable)
            .field("missing", &self.missing)
            .finish()
    }
}

impl Past {
    /// None shown, as `missing` says.
    pub fn missing(missing: Missing) -> Self {
        Self {
            missing: Some(missing),
            ..Self::default()
        }
    }

    /// The note before the turns shown, if any are: that they are as the transcript records
    /// them, and what was left out before them.
    pub fn before(&self) -> Option<String> {
        const SAID: &str = "Its earlier turns, as Claude Code's transcript records them";
        const WHOLE: &str = "session-search shows the whole conversation";
        if self.said.is_empty() {
            return None;
        }
        Some(match (self.cut, self.prompts_left_out) {
            (false, _) => format!("{SAID}."),
            (true, 0) => format!("{SAID}; turns before them are not shown here: {WHOLE}."),
            (true, 1) => format!("{SAID}; 1 turn before them is not shown here: {WHOLE}."),
            (true, count) => {
                format!("{SAID}; {count} turns before them are not shown here: {WHOLE}.")
            }
        })
    }

    /// The notes after the turns shown: lines that did not read, and why none was shown.
    pub fn after(&self) -> Vec<String> {
        let mut notes = Vec::new();
        match self.unreadable {
            0 => {}
            1 => notes.push("1 line of its transcript did not read, and is not shown.".into()),
            count => notes.push(format!(
                "{count} lines of its transcript did not read, and are not shown."
            )),
        }
        let why = match self.missing {
            None => return notes,
            Some(Missing::NotFound) => "its transcript was not found",
            Some(Missing::NotRead) => {
                "its transcript is not a regular file of this user's with one name, or did not read"
            }
            Some(Missing::Elsewhere) => "its transcript says it ran in another directory",
            Some(Missing::TooSlow) => "its transcript took too long to read",
            Some(Missing::TooMany) => "its transcript has too many lines to read",
        };
        notes.push(format!("Its earlier turns are not shown: {why}."));
        notes
    }
}

#[cfg(test)]
mod tests;
