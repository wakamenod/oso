//! What oso sent a native session and what the CLI replayed (ADR 0165): a message is the
//! person's only when oso sent its text and the CLI replayed it, or as a slash command the CLI
//! lists, which it never replays (step 2d), and a brief when an agent's tool started the
//! session with it; any other replay is a note, or nothing when a
//! command or hook wrapped it.
//!
//! Must not depend on: threads, files or the UI; methods of [`Rows`] over its own state.

use super::{Kind, Row, RowChange, Rows};
use crate::sessions::transcripts::WRAPPERS;

/// The most texts oso sent that wait for the CLI to replay them.
const MAX_SENT: usize = 64;

/// A command's output that the CLI replays as a message: shown as a note.
const COMMAND_OUTPUT: (&str, &str) = ("<local-command-stdout>", "</local-command-stdout>");

/// A text oso sent that waits for its replay.
#[derive(Debug)]
pub(super) struct Sent {
    text: String,
    /// Whose it is, when an agent's tool started the session with it.
    from: Option<String>,
}

impl Rows {
    /// Notes that oso sent `text` as the person's message: its replay is a prompt.
    pub fn sent(&mut self, text: &str) {
        self.wait_for(text, None);
    }

    /// Notes that oso sent `text` as the message an agent's tool started the session with: its
    /// replay is a brief, `from` who it is from.
    pub fn sent_brief(&mut self, text: &str, from: &str) {
        self.wait_for(text, Some(from.to_owned()));
    }

    fn wait_for(&mut self, text: &str, from: Option<String>) {
        if self.sent.len() == MAX_SENT {
            self.sent.pop_front();
        }
        let text = text.to_owned();
        self.sent.push_back(Sent { text, from });
    }

    /// oso sent `text` as a listed slash command, never replayed: its row is the person's now.
    pub fn sent_command(&mut self, text: &str) -> RowChange {
        self.push(Kind::Prompt(text.to_owned()), 0, false)
    }

    /// A message the CLI replayed: the person's when oso sent it, else a note or nothing.
    pub(super) fn replayed(&mut self, text: String) -> Option<RowChange> {
        if let Some(at) = self.sent.iter().position(|sent| sent.text == text) {
            // What was sent before it and never replayed, such as a command, is dropped.
            let from = self
                .sent
                .drain(..=at)
                .next_back()
                .and_then(|sent| sent.from);
            let kind = match from {
                Some(from) => Kind::Brief { from, text },
                None => Kind::Prompt(text),
            };
            return Some(self.push(kind, 0, false));
        }
        self.not_sent(text)
    }

    /// A message that is not the person's: a command's output is a note, as is any other
    /// message, and one a command or hook wrapped is nothing.
    pub(super) fn not_sent(&mut self, text: String) -> Option<RowChange> {
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
