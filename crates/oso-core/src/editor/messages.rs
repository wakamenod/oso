//! The messages the editor keeps, the echo area's line and `*Messages*` (ADR 0053,
//! ADR 0063).
//!
//! Must not depend on: `command`, `keymap`, or the host's window, threads or I/O.
//!
//! [`Editor::message`] reaches the echo area, `*Messages*` and the log.
//! [`Editor::echo`] reaches the echo area alone, for what must stay out of the other
//! two. The echo area keeps its text until the next key, and what was echoed around
//! a buffer that may hold secrets goes as soon as no such buffer is shown.

use super::{Editor, Selected};
use crate::buffer::{BufferId, Replacement};
use crate::display_map::DisplayPoint;
use crate::selection::SelectionSet;
use crate::sum_tree::Bias;
use crate::window::Scroll;

/// The messages the editor keeps, as Emacs's `message-log-max`.
pub(super) const MAX_MESSAGES: usize = 1000;

/// The longest message, in bytes: room for the longest path a message names on the
/// platforms oso runs on, and for what it says about it. A longer one is cut, so that
/// nothing can make the log or `*Messages*` grow faster than this.
pub const MAX_MESSAGE_BYTES: usize = 4096;

/// What ends a message that was cut.
const CUT: char = '…';

/// A message as the log and `*Messages*` show it: control and non-printable
/// characters are escaped, so that a name in it cannot move a terminal's cursor,
/// reorder the line, or add a line of its own. Quotes and backslashes stay as they
/// are.
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

/// `text`, cut to [`MAX_MESSAGE_BYTES`] on a character boundary.
fn capped(mut text: String) -> String {
    if text.len() > MAX_MESSAGE_BYTES {
        let mut end = MAX_MESSAGE_BYTES - CUT.len_utf8();
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push(CUT);
    }
    text
}

/// The echo area's line, until the next key.
pub(super) struct Echo {
    text: String,
    /// It says something of a buffer that may hold secrets, so it shows only while
    /// one is shown.
    withheld: bool,
}

/// Where a window was before it showed `*Messages*`, to go back to.
pub(super) struct Place {
    buffer: BufferId,
    selections: SelectionSet,
    scroll: Scroll,
}

impl Editor {
    /// Adds a message, for the echo area and `*Messages*`, and writes it to the
    /// log. Save completion and recovery offers arrive outside commands, so it
    /// is here and not on [`Ctx`](crate::command::Ctx). Messages reach the log and
    /// a client of the control socket: they must never quote buffer text (ADR 0028).
    /// One longer than [`MAX_MESSAGE_BYTES`] is cut.
    pub fn message(&mut self, text: impl Into<String>) {
        let text = capped(text.into());
        let shown = log_text(&text);
        log::info!("{shown}");
        let full = self.messages.len() == MAX_MESSAGES;
        if full {
            self.messages.pop_front();
        }
        self.add_to_messages_buffer(&shown, full);
        self.echo = Some(Echo {
            text: text.clone(),
            withheld: false,
        });
        self.messages.push_back(text);
    }

    /// Shows `text` in the echo area alone, for what must stay out of the log and
    /// `*Messages*`, such as keys the user typed (ADR 0056). If a buffer that may hold
    /// secrets is shown, it goes when no such buffer is.
    pub fn echo(&mut self, text: impl Into<String>) {
        self.echo = Some(Echo {
            text: capped(text.into()),
            withheld: self.shows_secret(),
        });
    }

    /// As [`Editor::echo`], for what a command said around a buffer that may hold
    /// secrets: where a motion stopped says where the cursor is (ADR 0058). It shows
    /// only while such a buffer is shown.
    pub fn echo_withheld(&mut self, text: impl Into<String>) {
        self.echo = Some(Echo {
            text: capped(text.into()),
            withheld: true,
        });
    }

    /// Empties the echo area. The next key does this, as in Emacs.
    pub fn clear_echo(&mut self) {
        self.echo = None;
    }

    /// Sets what is being typed, such as a prefix argument or a prefix key, which
    /// the echo area shows ahead of anything else until it is empty again.
    pub fn set_typing(&mut self, text: &str) {
        self.typing.clear();
        self.typing.push_str(text);
    }

    /// What the echo area shows.
    pub(super) fn echo_text(&self) -> &str {
        if !self.typing.is_empty() {
            return &self.typing;
        }
        match &self.echo {
            Some(echo) if !echo.withheld || self.shows_secret() => &echo.text,
            _ => "",
        }
    }

    /// The messages kept, oldest first; the last is the newest.
    pub fn messages(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.messages.iter().map(String::as_str)
    }

    /// The buffer behind `*Messages*`.
    pub fn messages_buffer(&self) -> BufferId {
        self.messages_buffer
    }

    /// Shows `*Messages*` in the selected window, with its last row in view. From
    /// `*Messages*`, goes back to where the window was.
    pub fn toggle_messages(&mut self) {
        let window = self.window();
        if window.buffer() != self.messages_buffer {
            self.before_messages = Some(Place {
                buffer: window.buffer(),
                selections: window.selections.clone(),
                scroll: window.scroll(),
            });
            self.show(self.messages_buffer);
            let (window, buffer) = self.current_mut();
            window.selections = SelectionSet::new(buffer.text().len());
            return;
        }
        let back = self.before_messages.take();
        match back.filter(|place| self.buffer(place.buffer).is_some()) {
            Some(place) => {
                self.show(place.buffer);
                self.restore_positions(place.selections, place.scroll);
            }
            None => {
                let scratch = self.scratch();
                self.show(scratch);
            }
        }
    }

    /// Adds a row to `*Messages*`, and takes the oldest one out if `full`. The text
    /// has no line break, as [`log_text`] escapes them, so each message is one row.
    /// Cursors at the end stay at the end.
    fn add_to_messages_buffer(&mut self, shown: &str, full: bool) {
        let id = self.messages_buffer;
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let display = buffer.display();
        let end = buffer.text().len();
        let oldest = if full {
            display.display_point_to_offset(DisplayPoint::new(1, 0), Bias::Left)
        } else {
            0
        };
        let line = format!("{shown}\n");
        let edits = [
            Replacement {
                range: 0..oldest,
                text: "",
            },
            Replacement {
                range: end..end,
                text: &line,
            },
        ];
        let Some(staged) = buffer.stage_internal(&edits) else {
            return;
        };
        let moved = self.moved_selections(id, &staged.display(), staged.steps(), Selected::Edited);
        // Nothing past this point can panic part way.
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.commit_internal(staged);
        }
        for (index, set) in moved {
            self.windows[index].selections = set;
        }
        self.messages_added = self.messages_added.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, EditError, ReadOnly};
    use crate::editor::MESSAGES;
    use std::path::PathBuf;

    fn messages_text(editor: &Editor) -> String {
        let id = editor.messages_buffer();
        editor.buffer(id).unwrap().text().to_string()
    }

    fn echo(editor: &Editor) -> &str {
        editor.echo_text()
    }

    /// An editor showing a file that may hold secrets.
    fn on_a_secret() -> Editor {
        Editor::with_buffer(Buffer::visiting(PathBuf::from("/tmp/.env")))
    }

    #[test]
    fn every_message_is_a_row_of_messages() {
        let mut editor = Editor::default();
        editor.message("Saved");
        editor.message("cannot open a\nb\u{202e}c");
        assert_eq!(
            messages_text(&editor),
            "Saved\ncannot open a\\nb\\u{202e}c\n"
        );
        let id = editor.messages_buffer();
        let buffer = editor.buffer(id).unwrap();
        assert_eq!(buffer.name(), MESSAGES);
        assert_eq!(buffer.display().row_count(), 3);
        assert!(
            !buffer.display().has_bidi_controls(),
            "a name's are escaped"
        );
        assert_eq!(
            editor.messages().next_back(),
            Some("cannot open a\nb\u{202e}c"),
            "the echo area escapes them as it draws"
        );
    }

    #[test]
    fn the_messages_buffer_keeps_as_many_rows_as_there_are_messages() {
        let mut editor = Editor::default();
        for index in 0..MAX_MESSAGES + 3 {
            editor.message(format!("{index}"));
        }
        let text = messages_text(&editor);
        assert_eq!(text.lines().count(), MAX_MESSAGES);
        assert_eq!(text.lines().next(), Some("3"));
        assert_eq!(text.lines().last(), Some("1002"));
    }

    #[test]
    fn an_internal_buffer_is_nobodys_work() {
        let mut editor = Editor::default();
        editor.message("Saved");
        assert_eq!(editor.buffers().count(), 1, "only *scratch*");
        let id = editor.messages_buffer();
        let buffer = editor.buffer(id).unwrap();
        assert!(buffer.is_internal());
        assert!(!buffer.is_modified());
        assert_eq!(buffer.read_only(), Some(ReadOnly::Internal));
        assert_eq!(
            editor.confirm_exit(crate::editor::ExitAction::Command),
            Ok(())
        );
    }

    #[test]
    fn a_key_cannot_edit_it_or_undo_it_or_kill_it() {
        let mut editor = Editor::default();
        editor.message("Saved");
        let id = editor.messages_buffer();
        let edit = Replacement {
            range: 0..0,
            text: "x",
        };
        assert_eq!(
            editor.apply_edits(id, &[edit]),
            Err(EditError::ReadOnly(ReadOnly::Internal))
        );
        editor.toggle_messages();
        assert_eq!(editor.undo(), Err(EditError::ReadOnly(ReadOnly::Internal)));
        editor.kill_buffer(id);
        assert_eq!(messages_text(&editor), "Saved\n", "it stays");
        assert_eq!(editor.window().buffer(), id);
    }

    #[test]
    fn a_window_that_shows_it_follows_a_new_message_or_stays_where_it_was() {
        let mut editor = Editor::default();
        editor.message("one");
        editor.toggle_messages();
        assert_eq!(editor.window().selections.primary().cursor, 4);
        editor.message("two");
        assert_eq!(editor.window().selections.primary().cursor, 8);
        editor.current_mut().0.selections = SelectionSet::new(1);
        editor.message("three");
        assert_eq!(editor.window().selections.primary().cursor, 1);
    }

    #[test]
    fn an_echo_stays_out_of_the_messages() {
        let mut editor = Editor::default();
        editor.echo("C-x a is undefined");
        assert_eq!(echo(&editor), "C-x a is undefined");
        assert_eq!(editor.messages().count(), 0);
        assert_eq!(messages_text(&editor), "");
        editor.clear_echo();
        assert_eq!(echo(&editor), "");
    }

    #[test]
    fn what_was_echoed_around_a_secret_goes_when_it_does() {
        for withheld in [true, false] {
            let mut editor = on_a_secret();
            let secret = editor.window().buffer();
            if withheld {
                editor.echo_withheld("End of buffer");
            } else {
                editor.echo("End of buffer");
            }
            assert_eq!(
                echo(&editor),
                "End of buffer",
                "for the person at the screen"
            );
            let other = editor.add_buffer(Buffer::new("other"));
            editor.show(other);
            assert_eq!(echo(&editor), "", "with the secret out of view");
            editor.show(secret);
            assert_eq!(echo(&editor), "End of buffer");
            assert_eq!(editor.messages().count(), 0);
        }
    }

    #[test]
    fn a_message_about_a_secret_buffer_stays_when_it_goes() {
        let mut editor = on_a_secret();
        editor.message("Wrote /tmp/.env");
        let other = editor.add_buffer(Buffer::new("other"));
        editor.show(other);
        assert_eq!(echo(&editor), "Wrote /tmp/.env");
    }
}
