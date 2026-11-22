//! What is logged of a review thread's reports and readings (ADR 0122): their kinds, paths
//! and counts, never a file's text, a comment's, nor a title or branch, which are anyone's words.
//!
//! Must not depend on: anything but the reports' types.

use std::fmt;

use super::{Content, Reading, Report};

impl fmt::Debug for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Listed {
                generation,
                entries,
                more,
                cut,
            } => f
                .debug_struct("Listed")
                .field("generation", generation)
                .field("entries", &entries.len())
                .field("more", more)
                .field("cut", cut)
                .finish(),
            Self::File {
                generation,
                reading,
            } => f
                .debug_struct("File")
                .field("generation", generation)
                .field("reading", reading)
                .finish(),
            Self::Read {
                generation,
                stopped,
            } => f
                .debug_struct("Read")
                .field("generation", generation)
                .field("stopped", stopped)
                .finish(),
            Self::Baseline { tree, left_out } => f
                .debug_struct("Baseline")
                .field("tree", tree)
                .field("left_out", left_out)
                .finish(),
            Self::BaselineGone => f.write_str("BaselineGone"),
            Self::Stale => f.write_str("Stale"),
            Self::Carried { cut_short, .. } => f
                .debug_struct("Carried")
                .field("cut_short", cut_short)
                .finish_non_exhaustive(),
            // A patch holds the file's text: only whether it went in.
            Self::Staged { ticket, staged } => f
                .debug_struct("Staged")
                .field("ticket", ticket)
                .field("applied", &staged.is_ok())
                .finish_non_exhaustive(),
            Self::Failed { asked, why } => f
                .debug_struct("Failed")
                .field("asked", asked)
                .field("why", why)
                .finish(),
            // A title and a branch are anyone's words.
            Self::PrFound(found) => f
                .debug_struct("PrFound")
                .field("number", &found.number)
                .field("head_checked_out", &found.head_checked_out)
                .finish_non_exhaustive(),
            Self::Fetching { number, .. } => f
                .debug_struct("Fetching")
                .field("number", number)
                .finish_non_exhaustive(),
            Self::Jumped(jumped) => f.debug_tuple("Jumped").field(jumped).finish(),
            Self::Commits(listed) => f
                .debug_struct("Commits")
                .field("commits", &listed.commits.len())
                .field("more", &listed.more)
                .finish(),
            Self::Since(found) => f
                .debug_struct("Since")
                .field("new", &found.new.len())
                .field("rebased", &found.rebased)
                .field("gone", &found.gone)
                .finish_non_exhaustive(),
            Self::Message(message) => f
                .debug_struct("Message")
                .field("lines", &message.lines.len())
                .finish_non_exhaustive(),
        }
    }
}

impl fmt::Debug for Reading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.content {
            Content::Text { shape, .. } => format!("text, {} hunks", shape.hunks().len()),
            Content::Binary { .. } => "binary".into(),
            Content::TooLarge { .. } => "too large".into(),
            Content::OutOfTime => "out of time".into(),
            Content::Withheld => "withheld".into(),
            Content::Conflicted => "conflicted".into(),
            Content::Submodule => "submodule".into(),
            Content::Unreadable(why) => format!("unreadable: {why}"),
        };
        f.debug_struct("Reading")
            .field("path", &String::from_utf8_lossy(&self.entry.path))
            .field("kind", &kind)
            .finish()
    }
}
