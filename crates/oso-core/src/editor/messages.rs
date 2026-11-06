//! The messages the editor keeps for the echo area and `*Messages*` (ADR 0053).
//!
//! Must not depend on: `command`, `keymap`, or the host's window, threads or I/O.

use super::Editor;

/// The messages the editor keeps, as Emacs's `message-log-max`.
pub(super) const MAX_MESSAGES: usize = 1000;

/// A message as the log shows it: control and non-printable characters are
/// escaped, so that a name in it cannot move a terminal's cursor or reorder the
/// line. Quotes and backslashes stay as they are.
pub(super) fn log_text(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for part in text.split_inclusive(['\'', '"', '\\']) {
        let (body, last) = match part.chars().next_back() {
            Some(last @ ('\'' | '"' | '\\')) => (&part[..part.len() - 1], Some(last)),
            _ => (part, None),
        };
        shown.extend(body.escape_debug());
        shown.extend(last);
    }
    shown
}

impl Editor {
    /// Adds a message, for the echo area and `*Messages*`, and writes it to the
    /// log. Save completion and recovery offers arrive outside commands, so it
    /// is here and not on [`Ctx`](crate::command::Ctx). Messages reach the log:
    /// they must never quote buffer text (ADR 0028).
    pub fn message(&mut self, text: impl Into<String>) {
        let text = text.into();
        log::info!("{}", log_text(&text));
        if self.messages.len() == MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(text);
    }

    /// The messages kept, oldest first; the last is the newest.
    pub fn messages(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.messages.iter().map(String::as_str)
    }
}
