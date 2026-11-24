//! A native session's earlier turns among its rows (ADR 0165, step 5): what its transcript
//! said, folded in as the stream's events are, before anything its process says.
//!
//! Must not depend on: threads, files or the UI; methods of [`Rows`] over its own state.
//!
//! - A message the transcript marks as the person's is a prompt, or the brief an agent's tool
//!   started the session with, the first one only; a message from another session a note; any other is what its replay would show: a command's output a note, a
//!   wrapped one nothing. One from claude.ai is a note that says so (ADR 0181).
//! - Replies, thinking and tool calls are rows as they are live, folded and grouped by the
//!   same rules. A call whose result the transcript does not hold is unfinished.
//! - No turn has a result row: the transcript keeps none.
//! - The ids of the session's own newest messages, live or read, are kept, at most
//!   [`MAX_KEPT`]: a later start reads only what its transcript holds after them (part 5b).

use super::{Changes, Kind, RowChange, Rows};
use crate::sessions::native::wire::transcript::Said;
use crate::sessions::transcripts::WRAPPERS;
use crate::sessions::worktree::brief::{self, Form};

/// The most ids of the session's newest messages kept.
pub const MAX_KEPT: usize = 64;

impl Rows {
    /// Whether any row is of the conversation: a note of oso's, or a line that did not read,
    /// is not.
    pub fn holds_conversation(&self) -> bool {
        (self.rows.iter()).any(|row| !matches!(row.kind, Kind::Note(_) | Kind::Unreadable(_)))
    }

    /// The ids of the session's own newest messages, the newest first: those its transcript's
    /// later turns follow on (step 5).
    pub fn kept_ids(&self) -> Vec<String> {
        self.kept.iter().rev().cloned().collect()
    }

    /// Keeps `uuid`, a line's id, if it is the session's own.
    pub(super) fn keep(&mut self, uuid: Option<String>, own: bool) {
        let Some(uuid) = uuid.filter(|_| own) else {
            return;
        };
        if self.kept.len() == MAX_KEPT {
            self.kept.pop_front();
        }
        self.kept.push_back(uuid);
    }

    /// Folds in `said`, a session's earlier turns, oldest first, and says which rows changed:
    /// once each, in their order.
    /// `first` says whether the conversation's first message is among them.
    pub fn past(&mut self, said: Vec<Said>, mut first: bool) -> Vec<RowChange> {
        let mut changes = Changes::default();
        for said in said {
            let changed = match said {
                Said::Prompt { text, uuid } => {
                    self.keep(uuid, true);
                    self.past_prompt(text, &mut first).into_iter().collect()
                }
                // Never the person's here nor a brief (ADR 0181), and the first message if it
                // is: no later one is a brief then.
                Said::Remote { text, uuid } => {
                    first = false;
                    self.keep(uuid, true);
                    vec![self.remote(&text)]
                }
                Said::Note { text, uuid } => {
                    self.keep(uuid, true);
                    vec![self.note(&text)]
                }
                Said::User(mut message) => {
                    let text = (message.text.take())
                        .filter(|_| !message.synthetic && message.parent.is_none());
                    let mut changed = self.user(message);
                    changed.extend(text.and_then(|text| self.not_sent(text)));
                    changed
                }
                Said::Assistant(message) => self.assistant(message),
                Said::Compacted { pre, post, uuid } => {
                    self.keep(uuid, true);
                    vec![self.push(Kind::Compacted { pre, post }, 0, false)]
                }
            };
            for change in changed {
                changes.add(change);
            }
        }
        // No process runs them now.
        for change in self.unfinish_turn() {
            changes.add(change);
        }
        changes.into_iter().collect()
    }

    /// Notes whose the session's first message is when an agent's tool started it, for the
    /// next read of its transcript.
    pub fn note_started_by(&mut self, from: Option<String>) {
        self.started_by = from;
    }

    /// A message the transcript marks as the person's, the conversation's `first` or not: a
    /// prompt, unless a command or a hook wrapped it, or it is the brief the session started
    /// with. The transcript does not say whose it is: it is a brief when it starts as oso
    /// starts one, and one of a worktree, which the person hands off too, only where an agent's
    /// tool started the session.
    fn past_prompt(&mut self, text: String, first: &mut bool) -> Option<RowChange> {
        let start = text.trim_start();
        if WRAPPERS.iter().any(|wrapper| start.starts_with(wrapper)) {
            return self.not_sent(text);
        }
        let from = match std::mem::take(first).then(|| brief::form(start)).flatten() {
            Some(Form::Directory) => Some(self.started_by.take().unwrap_or_else(brief::unknown)),
            Some(Form::Worktree) => self.started_by.take(),
            None => None,
        };
        let kind = match from {
            Some(from) => Kind::Brief { from, text },
            None => Kind::Prompt(text),
        };
        Some(self.push(kind, 0, false))
    }
}
