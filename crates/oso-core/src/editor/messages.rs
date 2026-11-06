//! The messages the editor keeps, the echo area's line and `*Messages*`, and going to
//! and from that buffer (ADR 0053, ADR 0063).
//!
//! Must not depend on: `command`, `keymap`, or the host's window, threads or I/O.
//!
//! [`Editor::message`] reaches the echo area, `*Messages*` and the log. [`Editor::echo`]
//! reaches the echo area alone, for what must stay out of the other two. So does a
//! message from a command run around a buffer that may hold secrets, whatever the
//! command is, since what it says can depend on the text. The echo area keeps its
//! text until the next key, and what was echoed around such a buffer goes as soon as
//! it is out of view.

use std::panic::{self, AssertUnwindSafe};

use super::{Editor, Selected, clip_cursors};
use crate::buffer::{BufferId, Replacement};
use crate::selection::SelectionSet;
use crate::window::Scroll;

/// The messages the editor keeps, as Emacs's `message-log-max`.
pub(super) const MAX_MESSAGES: usize = 1000;

/// The longest message, in bytes, and the longest row of `*Messages*`: room for what
/// a message says and the longest path most systems have. Messages say what happened
/// before they name a path, so that a cut takes the path's end. Nothing can make the
/// log or `*Messages*` grow faster than this.
pub(super) const MAX_MESSAGE_BYTES: usize = 4096;

/// What ends a text that was cut.
pub(super) const CUT: char = '…';

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
    /// It was said around a buffer that may hold secrets, so it shows only while one
    /// is shown, and goes when the window moves on.
    withheld: bool,
}

/// Where a window was before it showed `*Messages*`, to go back to. M2-19 removes this
/// with the way back, once `C-x b` leaves that buffer.
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
    /// A long one is cut, so what it says comes before any path it names.
    ///
    /// From a command run around a buffer that may hold secrets, this only reaches the
    /// echo area, as [`Editor::echo`] does, or nothing if that buffer has left the screen.
    /// What a command says can depend on the text: "End of buffer" says where a cursor is.
    pub fn message(&mut self, text: impl Into<String>) {
        if self.shows_secret() && self.undo_scope.is_some() {
            self.echo(text);
            return;
        }
        if self.command_began_on_a_secret() {
            return;
        }
        let text = capped(text.into());
        let shown = capped(log_text(&text));
        log::info!("{shown}");
        let full = self.messages.len() == MAX_MESSAGES;
        if full {
            self.messages.pop_front();
        }
        self.messages.push_back(text.clone());
        self.echo = Some(Echo {
            text,
            withheld: false,
        });
        // This comes last, and a panic here costs a row: a message reports what went
        // wrong, also just after a panic, outside the boundary that catches those.
        let added = panic::catch_unwind(AssertUnwindSafe(|| {
            self.add_to_messages_buffer(&shown, full);
        }));
        if added.is_err() {
            log::error!("a message did not reach *Messages*");
        }
    }

    /// Whether a command is running that began with a buffer that may hold secrets in
    /// the selected window.
    fn command_began_on_a_secret(&self) -> bool {
        self.undo_scope
            .as_ref()
            .and_then(|scope| self.buffer(scope.before.0))
            .is_some_and(|buffer| buffer.is_secret())
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

    /// Empties the echo area. The next key does this, as in Emacs.
    pub fn clear_echo(&mut self) {
        self.echo = None;
    }

    /// Forgets what was echoed around a buffer that may hold secrets, as the window
    /// moves on, so that it does not come back with the next such buffer.
    pub(super) fn forget_withheld_echo(&mut self) {
        if self.echo.as_ref().is_some_and(|echo| echo.withheld) {
            self.echo = None;
        }
    }

    /// Sets the keys in progress, a prefix argument and a prefix key, which the echo
    /// area shows ahead of anything else until they are empty again.
    pub fn set_keys_in_progress(&mut self, keys: &str) {
        self.keys_in_progress.clear();
        self.keys_in_progress.push_str(keys);
    }

    /// What the echo area shows.
    pub(super) fn echo_text(&self) -> &str {
        if !self.keys_in_progress.is_empty() {
            return &self.keys_in_progress;
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
            self.follow_cursor();
            return;
        }
        let back = self.before_messages.take();
        match back.filter(|place| self.buffer(place.buffer).is_some()) {
            Some(Place {
                buffer,
                mut selections,
                scroll,
            }) => {
                self.show(buffer);
                // The buffer may have changed behind `*Messages*`.
                clip_cursors(&mut selections, &self.current().1.display(), &[]);
                self.restore_positions(selections, scroll);
            }
            None => {
                let scratch = self.scratch();
                self.show(scratch);
            }
        }
    }

    /// Adds a row to `*Messages*`, and takes the oldest one out if `full`. The text
    /// has no line break, as [`log_text`] escapes them, so each message is one row.
    /// Cursors at the end stay at the end, and the view follows the selected window's.
    fn add_to_messages_buffer(&mut self, shown: &str, full: bool) {
        let id = self.messages_buffer;
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let end = buffer.text().len();
        let oldest = if full { buffer.text().line_start(1) } else { 0 };
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
        if self.window().buffer() == id {
            self.follow_cursor();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, EditError, ReadOnly};
    use crate::editor::MESSAGES;
    use crate::selection::Selection;
    use crate::window::ViewSize;
    use std::path::PathBuf;

    fn messages_text(editor: &Editor) -> String {
        let id = editor.messages_buffer;
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
        let id = editor.messages_buffer;
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
        let id = editor.messages_buffer;
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
        let id = editor.messages_buffer;
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
    fn what_was_echoed_around_a_secret_goes_for_good_when_the_window_moves_on() {
        let mut editor = on_a_secret();
        let secret = editor.window().buffer();
        editor.echo("End of buffer");
        assert_eq!(
            echo(&editor),
            "End of buffer",
            "for the person at the screen"
        );
        let other = editor.add_buffer(Buffer::new("other"));
        editor.show(other);
        assert_eq!(echo(&editor), "", "with the secret out of view");
        editor.show(secret);
        assert_eq!(echo(&editor), "", "it does not come back");
        assert_eq!(editor.messages().count(), 0);
    }

    #[test]
    fn what_was_echoed_around_a_secret_goes_with_a_killed_buffer() {
        let mut editor = on_a_secret();
        let secret = editor.window().buffer();
        editor.echo("End of buffer");
        editor.kill_buffer(secret);
        assert_eq!(echo(&editor), "");
    }

    #[test]
    fn a_message_from_a_command_around_a_secret_stays_in_the_echo_area() {
        let mut editor = on_a_secret();
        editor.begin_undo_group();
        editor.message("End of buffer");
        editor.end_undo_group();
        assert_eq!(echo(&editor), "End of buffer");
        assert_eq!(editor.messages().count(), 0);
        assert_eq!(messages_text(&editor), "");
    }

    #[test]
    fn a_message_from_a_command_that_left_a_secret_goes_nowhere() {
        let mut editor = on_a_secret();
        let other = editor.add_buffer(Buffer::new("other"));
        editor.begin_undo_group();
        editor.show(other);
        editor.message("End of buffer");
        editor.end_undo_group();
        assert_eq!(echo(&editor), "");
        assert_eq!(editor.messages().count(), 0);
        assert_eq!(messages_text(&editor), "");
    }

    #[test]
    fn a_message_from_outside_a_command_is_kept_around_a_secret() {
        let mut editor = on_a_secret();
        editor.message("Wrote /tmp/.env");
        assert_eq!(editor.messages().count(), 1);
        assert_eq!(messages_text(&editor), "Wrote /tmp/.env\n");
    }

    #[test]
    fn a_message_and_its_row_are_capped_on_a_character_boundary() {
        let mut editor = Editor::default();
        editor.message("日".repeat(MAX_MESSAGE_BYTES));
        let newest = editor.messages().next_back().unwrap();
        assert!(newest.len() <= MAX_MESSAGE_BYTES);
        assert!(newest.ends_with(CUT));
        editor.echo("x".repeat(MAX_MESSAGE_BYTES * 2));
        assert_eq!(echo(&editor).len(), MAX_MESSAGE_BYTES);

        // A control character takes six bytes as it is escaped.
        let mut editor = Editor::default();
        editor.message("\u{7f}".repeat(MAX_MESSAGE_BYTES));
        let text = messages_text(&editor);
        assert!(text.len() <= MAX_MESSAGE_BYTES + 1, "{}", text.len());
    }

    #[test]
    fn the_view_follows_a_message_from_outside_a_command() {
        let mut editor = Editor::default();
        editor.set_view_size(ViewSize::new(3, 40));
        editor.toggle_messages();
        for index in 0..10 {
            editor.message(format!("{index}"));
        }
        assert_eq!(
            editor.window().scroll().top,
            8,
            "the empty last row in view"
        );
    }

    #[test]
    fn it_comes_back_to_where_the_window_was_with_every_cursor() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", "abcdef"));
        let cursors = vec![Selection::at(1), Selection::at(3), Selection::at(5)];
        let set = SelectionSet::from_parts(cursors, 1).unwrap();
        editor.current_mut().0.selections = set.clone();
        editor.message("one");
        editor.toggle_messages();
        assert_eq!(editor.current().1.name(), MESSAGES);
        assert_eq!(editor.window().selections.primary().cursor, 4, "at the end");
        editor.toggle_messages();
        assert_eq!(editor.current().1.name(), "t");
        assert_eq!(editor.window().selections, set);
    }

    #[test]
    fn it_comes_back_to_scratch_if_the_buffer_has_gone() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", "abc"));
        let id = editor.window().buffer();
        editor.toggle_messages();
        editor.kill_buffer(id);
        editor.toggle_messages();
        assert_eq!(editor.current().1.name(), crate::editor::SCRATCH);
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
