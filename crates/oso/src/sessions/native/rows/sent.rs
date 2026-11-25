//! What oso sent a native session and what the CLI replayed (ADR 0165): a message is the
//! person's only when oso sent its text and the CLI replayed it, or as a slash command the CLI
//! lists, which it never replays (step 2d), and a brief when an agent's tool started the
//! session with it; any other replay is a note, or nothing when a
//! command or hook wrapped it. One that waits for its replay is no row (`render::sending_rows`
//! draws it); one whose replay will not come is a note that says so.
//!
//! Must not depend on: threads, files or the UI; methods of [`Rows`] over its own state.

use std::collections::VecDeque;

use super::{Kind, Row, RowChange, Rows};
use crate::sessions::native::render::one_line;
use crate::sessions::transcripts::WRAPPERS;

/// The most texts oso sent that wait for the CLI to replay them.
const MAX_SENT: usize = 8;

/// What starts the note of a message oso sent that no replay came of: a later one's came.
const PASSED: &str = "Not confirmed by claude: ";
/// The process ended first.
const ENDED: &str = "Not confirmed by claude, which ended: ";
/// So, and the person's text is in their composer again.
const BACK: &str = "Not confirmed by claude, which ended, and back in the composer: ";

/// What starts the note of a message from claude.ai.
const FROM_REMOTE: &str = "From claude.ai, not from oso: ";

/// A command's output that the CLI replays as a message: shown as a note.
const COMMAND_OUTPUT: (&str, &str) = ("<local-command-stdout>", "</local-command-stdout>");

/// The texts oso sent that wait for their replay, oldest first, and how often they changed.
#[derive(Debug, Default)]
pub(super) struct Sent {
    texts: VecDeque<Text>,
    version: u64,
    /// The version of the session's commands they were last looked up in.
    looked_up: u64,
}

/// A text oso sent.
#[derive(Debug)]
struct Text {
    text: String,
    /// Whose it is, when an agent's tool started the session with it.
    from: Option<String>,
    /// The person sent it: nothing a start sends is theirs to have back.
    persons: bool,
    /// A turn ended since, with no replay of it.
    ended: bool,
}

impl Text {
    /// The row it is once the CLI took it.
    fn row(self) -> Kind {
        let text = self.text;
        match self.from {
            Some(from) => Kind::Brief { from, text },
            None => Kind::Prompt(text),
        }
    }
}

/// A text oso sent that waits for its replay: of a brief, whose it is; whether the person sent it
/// and whether a turn ended since with no replay of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sending<'a> {
    pub text: &'a str,
    pub from: Option<&'a str>,
    pub persons: bool,
    pub ended: bool,
}

impl Rows {
    /// Notes that oso sent `text` as the person's message: its replay is a prompt. The note of
    /// one that waits no more, if too many do.
    #[must_use]
    pub fn sent(&mut self, text: &str) -> Option<RowChange> {
        self.wait_for(text, None, true)
    }

    /// Notes that oso sent `text` as the message a session started with, the person's or oso's
    /// own of their session: as [`Rows::sent`], but it never goes back to the composer.
    #[must_use]
    pub fn sent_first(&mut self, text: &str) -> Option<RowChange> {
        self.wait_for(text, None, false)
    }

    /// Notes that oso sent `text` as the message an agent's tool started the session with: its
    /// replay is a brief, `from` who it is from. As [`Rows::sent`].
    #[must_use]
    pub fn sent_brief(&mut self, text: &str, from: &str) -> Option<RowChange> {
        self.wait_for(text, Some(from.to_owned()), false)
    }

    fn wait_for(&mut self, text: &str, from: Option<String>, persons: bool) -> Option<RowChange> {
        let over = self.sent.texts.len() >= MAX_SENT;
        let passed = over.then(|| self.sent.texts.pop_front()).flatten();
        let text = text.to_owned();
        self.sent.texts.push_back(Text {
            text,
            from,
            persons,
            ended: false,
        });
        self.sent.version += 1;
        passed.map(|sent| self.unconfirmed(PASSED, &sent))
    }

    /// Whether a blank line sets a message added now apart from the row before.
    pub fn gap_next(&self) -> bool {
        !matches!(
            self.rows.last().map(|row| &row.kind),
            None | Some(Kind::Result(_))
        )
    }

    /// The texts oso sent that wait for their replay, oldest first.
    pub fn sending(&self) -> impl ExactSizeIterator<Item = Sending<'_>> {
        self.sent.texts.iter().map(|sent| Sending {
            text: &sent.text,
            from: sent.from.as_deref(),
            persons: sent.persons,
            ended: sent.ended,
        })
    }

    /// A turn ended: what waits was not replayed in it, and may be late.
    pub(super) fn sending_turn_ended(&mut self) {
        for sent in &mut self.sent.texts {
            if !sent.ended {
                sent.ended = true;
                self.sent.version += 1;
            }
        }
    }

    /// The session's commands are those of `version`: what waits that is one, as `runs` says, and
    /// whose turn has not ended, is never replayed, and its row is there, as
    /// [`Rows::sent_command`]'s. Looked up once a version.
    pub fn sending_commands(
        &mut self,
        version: u64,
        runs: impl Fn(&str) -> bool,
    ) -> Vec<RowChange> {
        let mut changes = Vec::new();
        if std::mem::replace(&mut self.sent.looked_up, version) == version {
            return changes;
        }
        let is_one = |sent: &Text| !sent.ended && runs(&sent.text);
        while let Some(at) = self.sent.texts.iter().position(is_one) {
            let Some(sent) = self.sent.texts.remove(at) else {
                break;
            };
            self.sent.version += 1;
            changes.push(self.push(sent.row(), 0, false));
        }
        changes
    }

    /// How often [`Rows::sending`] changed.
    pub fn sending_version(&self) -> u64 {
        self.sent.version
    }

    /// The person's newest text whose turn is under way, which goes back to their composer if the
    /// process ends: one whose turn ended may have run.
    pub fn sending_back(&self) -> Option<&str> {
        self.at_back().map(|at| self.sent.texts[at].text.as_str())
    }

    fn at_back(&self) -> Option<usize> {
        (self.sent.texts.iter()).rposition(|sent| sent.persons && !sent.ended)
    }

    /// The process ended: what waits is replayed no more, and each is a note that says so, and
    /// of [`Rows::sending_back`]'s whether it went `back`.
    pub fn sending_ended(&mut self, back: bool) -> Vec<RowChange> {
        let back = self.at_back().filter(|_| back);
        let sent = std::mem::take(&mut self.sent.texts);
        self.sent.version += 1;
        (sent.iter().enumerate())
            .map(|(at, sent)| {
                let said = if back == Some(at) { BACK } else { ENDED };
                self.unconfirmed(said, sent)
            })
            .collect()
    }

    /// The note of `sent`, which the CLI did not replay: what `said` says of it, and of a brief
    /// whose it is. The text's later lines are further in, so that none reads as another note.
    fn unconfirmed(&mut self, said: &str, sent: &Text) -> RowChange {
        let from = match &sent.from {
            Some(from) => format!("a message from {}: ", one_line(from)),
            None => String::new(),
        };
        let text = sent.text.replace('\n', "\n  ");
        let note = format!("{said}{from}{text}");
        self.push(Kind::Note(note), 0, false)
    }

    /// oso sent `text` as a listed slash command, never replayed: its row is the person's now.
    pub fn sent_command(&mut self, text: &str) -> RowChange {
        self.push(Kind::Prompt(text.to_owned()), 0, false)
    }

    /// A message the CLI replayed: the person's when oso sent it, after a note of each sent
    /// before it and not replayed; else a note or nothing.
    pub(super) fn replayed(&mut self, text: String) -> Vec<RowChange> {
        // Of two alike, the one whose turn may be under way.
        let alike = || (self.sent.texts.iter().enumerate()).filter(|(_, sent)| sent.text == text);
        let Some((at, _)) = (alike().find(|(_, sent)| !sent.ended)).or_else(|| alike().next())
        else {
            return self.not_sent(text).into_iter().collect();
        };
        self.sent.version += 1;
        let mut changes = Vec::new();
        for passed in self.sent.texts.drain(..at).collect::<Vec<_>>() {
            changes.push(self.unconfirmed(PASSED, &passed));
        }
        if let Some(sent) = self.sent.texts.pop_front() {
            changes.push(self.push(sent.row(), 0, false));
        }
        changes
    }

    /// A message from claude.ai (ADR 0181): a note that says so, whatever it holds, so that none
    /// reads as oso's or the CLI's words, or draws nothing.
    pub(super) fn remote(&mut self, text: &str) -> RowChange {
        self.push(Kind::Note(format!("{FROM_REMOTE}{text}")), 0, false)
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
