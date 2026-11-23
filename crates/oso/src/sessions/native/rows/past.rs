//! A native session's earlier turns among its rows (ADR 0165, step 5): what its transcript
//! said, folded in as the stream's events are, before anything its process says.
//!
//! Must not depend on: threads, files or the UI; methods of [`Rows`] over its own state.
//!
//! - A message the transcript marks as the person's is a prompt; a message from another
//!   session a note; any other is what its replay would show: a command's output a note, a
//!   wrapped one nothing.
//! - Replies, thinking and tool calls are rows as they are live, folded and grouped by the
//!   same rules. A call whose result the transcript does not hold is unfinished.
//! - No turn has a result row: the transcript keeps none.

use super::{Changes, Kind, RowChange, Rows};
use crate::sessions::native::wire::transcript::Said;
use crate::sessions::transcripts::WRAPPERS;

impl Rows {
    /// Whether any row is of the conversation: a note of oso's, or a line that did not read,
    /// is not.
    pub fn holds_conversation(&self) -> bool {
        (self.rows.iter()).any(|row| !matches!(row.kind, Kind::Note(_) | Kind::Unreadable(_)))
    }

    /// Folds in `said`, a session's earlier turns, oldest first, and says which rows changed:
    /// once each, in their order.
    pub fn past(&mut self, said: Vec<Said>) -> Vec<RowChange> {
        let mut changes = Changes::default();
        for said in said {
            let changed = match said {
                Said::Prompt(text) => self.past_prompt(text).into_iter().collect(),
                Said::Note(text) => vec![self.note(&text)],
                Said::User(mut message) => {
                    let text = (message.text.take())
                        .filter(|_| !message.synthetic && message.parent.is_none());
                    let mut changed = self.user(message);
                    changed.extend(text.and_then(|text| self.not_sent(text)));
                    changed
                }
                Said::Assistant(message) => self.assistant(message),
                Said::Compacted { pre, post } => {
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

    /// A message the transcript marks as the person's: a prompt, unless a command or a hook
    /// wrapped it.
    fn past_prompt(&mut self, text: String) -> Option<RowChange> {
        let start = text.trim_start();
        if WRAPPERS.iter().any(|wrapper| start.starts_with(wrapper)) {
            return self.not_sent(text);
        }
        Some(self.push(Kind::Prompt(text), 0, false))
    }
}
