//! What oso sent a native session and what the CLI replayed (ADR 0165): a message is the
//! person's only when oso sent its text and the CLI replayed it, or as a slash command the CLI
//! lists, which it never replays (step 2d); any other replay is a note, or nothing when a
//! command or hook wrapped it.
//!
//! Must not depend on: threads, files or the UI; methods of [`Rows`] over its own state.

use super::{Kind, Row, RowChange, Rows};
use crate::sessions::transcripts::WRAPPERS;

/// The most texts oso sent that wait for the CLI to replay them.
const MAX_SENT: usize = 64;

/// A command's output that the CLI replays as a message: shown as a note.
const COMMAND_OUTPUT: (&str, &str) = ("<local-command-stdout>", "</local-command-stdout>");

impl Rows {
    /// Notes that oso sent `text` as the person's message: its replay is a prompt.
    pub fn sent(&mut self, text: &str) {
        if self.sent.len() == MAX_SENT {
            self.sent.pop_front();
        }
        self.sent.push_back(text.to_owned());
    }

    /// oso sent `text` as a listed slash command, never replayed: its row is the person's now.
    pub fn sent_command(&mut self, text: &str) -> RowChange {
        self.push(Kind::Prompt(text.to_owned()), 0, false)
    }

    /// A message the CLI replayed: the person's when oso sent it, else a note or nothing.
    pub(super) fn replayed(&mut self, text: String) -> Option<RowChange> {
        if let Some(at) = self.sent.iter().position(|sent| *sent == text) {
            // What was sent before it and never replayed, such as a command, is dropped.
            self.sent.drain(..=at);
            return Some(self.push(Kind::Prompt(text), 0, false));
        }
        let trimmed = text.trim();
        if let Some(output) = trimmed
            .strip_prefix(COMMAND_OUTPUT.0)
            .and_then(|rest| rest.strip_suffix(COMMAND_OUTPUT.1))
        {
            let output = output.trim();
            // `/compact` says so twice: its boundary, then its output.
            let said = output == "Compacted"
                && matches!(
                    self.rows.last(),
                    Some(Row {
                        kind: Kind::Compacted { .. },
                        ..
                    })
                );
            return (!said && !output.is_empty())
                .then(|| self.push(Kind::Note(output.to_owned()), 0, false));
        }
        if WRAPPERS.iter().any(|start| trimmed.starts_with(start)) {
            return None;
        }
        Some(self.push(Kind::Note(text), 0, false))
    }
}
