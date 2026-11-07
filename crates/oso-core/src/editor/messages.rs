//! The messages the editor keeps, the echo area's line and `*Messages*`, and going to
//! and from that buffer (ADR 0053, ADR 0063).
//!
//! Must not depend on: `command`, `keymap`, or the host's window, threads or I/O.
//!
//! [`Editor::message`] reaches the echo area, `*Messages*` and the log. [`Editor::echo`]
//! reaches the echo area alone, for what must stay out of the other two. So does a
//! message from a command run around a buffer that may hold secrets, whatever the command
//! is, since what it says can depend on the text, and one from a command that took such a
//! buffer off the screen goes nowhere. The dispatcher marks where a command begins and ends.
//!
//! The echo area keeps its text until the next key. What was echoed around such a buffer,
//! or a file not yet judged, goes when the window moves on. A file judged ordinary keeps
//! it until the next key, as no text of it had arrived. What arrives behind keys in
//! progress waits for them to end, since they show in its place.

use super::{Editor, Selected};
use crate::buffer::{BufferId, Replacement};
use crate::motion;
use crate::selection::SelectionSet;
use crate::window::{Scroll, Window};

/// The messages the editor keeps, as Emacs's `message-log-max`.
const MAX_MESSAGES: usize = 1000;

/// The longest message, in bytes, and the longest row of `*Messages*`: room for what
/// a message says and the longest path most systems have. Messages say what happened
/// before they name a path, so that a cut takes the path's end. Nothing can make the
/// log or `*Messages*` grow faster than this.
const MAX_MESSAGE_BYTES: usize = 4096;

/// What ends a text that was cut.
pub(super) const CUT: char = '…';

/// A message as the log and `*Messages*` show it: control and non-printable
/// characters are escaped, so that a name in it cannot move a terminal's cursor,
/// reorder the line, or add a line of its own. Quotes and backslashes stay as they
/// are, and so does a full-width space, which ordinary Japanese file names hold and which
/// looks like what it is. A no-break space stays escaped, as nothing tells it from a space.
fn log_text(text: &str) -> String {
    const AS_THEY_ARE: [char; 4] = ['\'', '"', '\\', '\u{3000}'];
    let mut shown = String::with_capacity(text.len());
    for part in text.split_inclusive(AS_THEY_ARE) {
        let (body, last) = match part.chars().next_back() {
            Some(last) if AS_THEY_ARE.contains(&last) => {
                (&part[..part.len() - last.len_utf8()], Some(last))
            }
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
        let end = text.floor_char_boundary(MAX_MESSAGE_BYTES - CUT.len_utf8());
        text.truncate(end);
        text.push(CUT);
    }
    text
}

/// The echo area's line, until the next key.
pub(super) struct Echo {
    text: String,
    /// It was said around a buffer that may hold secrets, or a file not yet judged, so it
    /// goes when the window moves on, for good.
    goes_with_the_window: bool,
}

/// Where a window was before it showed an internal buffer, to go back to. M2-19 removes this
/// with the way back, once `C-x b` leaves those buffers.
pub(super) struct Place {
    pub(super) buffer: BufferId,
    pub(super) selections: SelectionSet,
    pub(super) scroll: Scroll,
}

impl Place {
    pub(super) fn of(window: &Window) -> Self {
        Self {
            buffer: window.buffer(),
            selections: window.selections.clone(),
            scroll: window.scroll(),
        }
    }
}

/// `text` as a row of an internal buffer shows it, escaped as in [`log_text`], so that a name
/// from anywhere stays on one row and cannot reorder it.
pub fn row_text(text: &str) -> String {
    log_text(text)
}

impl Editor {
    /// Adds a message, for the echo area and `*Messages*`, and writes it to the
    /// log. Save completion and recovery offers arrive outside commands, so it
    /// is here and not on [`Ctx`](crate::command::Ctx). Messages reach the log and
    /// a client of the control socket: they must never quote buffer text (ADR 0028).
    /// A long one is cut, so what it says comes before any path it names.
    ///
    /// From a command run around a buffer that may hold secrets, this only reaches the
    /// echo area, or nothing if the command took that buffer off the screen, as what a
    /// command says can depend on the text.
    pub fn message(&mut self, text: impl Into<String>) {
        if let Some(left_secret) = self.command_left_secret {
            if left_secret {
                return;
            }
            if self.shows_secret() {
                self.set_echo(text);
                return;
            }
        }
        let text = capped(text.into());
        let shown = capped(log_text(&text));
        log::info!("{shown}");
        self.echo = Some(Echo {
            text,
            goes_with_the_window: false,
        });
        // Last, so that the log and the echo area have it if this panics. Outside a command
        // that is a panic like any other: the hook writes the emergency copies, and the process
        // ends (ADR 0062). So nothing here may catch it. Inside one, the boundary catches it.
        self.add_to_messages_buffer(&shown);
    }

    /// Marks that a command begins. Until [`Editor::end_command`], what it says is
    /// held to what a buffer that may hold secrets allows.
    pub fn begin_command(&mut self) {
        self.command_left_secret = Some(false);
    }

    /// Marks that the command has ended, so that messages reach `*Messages*` and the log
    /// again. What a panic leaves comes after this: it names only the command, and is
    /// a bug to find.
    pub fn end_command(&mut self) {
        self.command_left_secret = None;
    }

    /// Shows `text` in the echo area alone, for what must stay out of the log and
    /// `*Messages*`, such as keys the user typed (ADR 0056). Around a buffer that may hold
    /// secrets, it goes when the window moves on. A command that took one off the screen
    /// says nothing, and what waits behind keys in progress is not replaced by what
    /// they echo, such as "Quit", which says less.
    pub fn echo(&mut self, text: impl Into<String>) {
        let waiting = !self.keys_in_progress.is_empty() && self.echo.is_some();
        if !waiting && self.command_left_secret != Some(true) {
            self.set_echo(text);
        }
    }

    fn set_echo(&mut self, text: impl Into<String>) {
        self.echo = Some(Echo {
            text: capped(text.into()),
            goes_with_the_window: self.shows_secret(),
        });
    }

    /// Empties the echo area, as the next key does in Emacs. While keys are in progress they
    /// show in its place, and what is there came after the key that began them and has not
    /// been seen, so it stays for when they end.
    pub fn clear_echo(&mut self) {
        if self.keys_in_progress.is_empty() {
            self.echo = None;
        }
    }

    /// A window is about to stop showing its buffer: what was echoed around one that may hold
    /// secrets goes, and does not come back with the next such buffer, and a command
    /// running then has left it.
    pub(super) fn window_moves_on(&mut self) {
        if self
            .echo
            .as_ref()
            .is_some_and(|echo| echo.goes_with_the_window)
        {
            self.echo = None;
        }
        if self.command_left_secret.is_some() && self.shows_secret() {
            self.command_left_secret = Some(true);
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
        self.echo.as_ref().map_or("", |echo| &echo.text)
    }

    /// The internal buffer that keeps every message, one per row, oldest first, as the
    /// log shows them.
    pub fn messages_buffer(&self) -> BufferId {
        self.messages_buffer
    }

    /// Where the selected window goes back to through the internal buffers it has gone to, if
    /// it is on one and that place still exists, for a state handoff.
    pub fn place_behind_internal(&self) -> Option<(BufferId, &SelectionSet, Scroll)> {
        if !self.current().1.is_internal() {
            return None;
        }
        let place = self.way_back.first()?;
        let buffer = self.buffer(place.buffer)?;
        (!buffer.is_internal()).then_some((place.buffer, &place.selections, place.scroll))
    }

    /// Shows the internal buffer `id`, to come back from later. If the window has already come
    /// through it, this goes back to it, so that going between two of them cannot grow the way.
    pub(super) fn go_to_internal(&mut self, id: BufferId) {
        match self.way_back.iter().position(|place| place.buffer == id) {
            Some(index) => self.way_back.truncate(index),
            None => self.way_back.push(Place::of(self.window())),
        }
        self.show(id);
    }

    /// Goes back to where the window was before the internal buffer it is on.
    pub(super) fn leave_internal(&mut self) {
        // A place whose buffer has gone is no way back, and the one before may still be.
        while let Some(place) = self.way_back.pop() {
            if self.buffer(place.buffer).is_some() {
                self.go_back(Some(place));
                return;
            }
        }
        self.go_back(None);
    }

    /// Shows `*Messages*` in the selected window, with its last row in view. From
    /// `*Messages*`, goes back to where the window was.
    pub fn toggle_messages(&mut self) {
        if self.window().buffer() != self.messages_buffer {
            self.go_to_internal(self.messages_buffer);
            let (window, buffer) = self.current_mut();
            window.selections = SelectionSet::new(buffer.text().len());
            self.follow_cursor();
            return;
        }
        self.leave_internal();
    }

    /// Shows `back` as it was, or `*scratch*` if there is no such place any more.
    fn go_back(&mut self, back: Option<Place>) {
        match back.filter(|place| self.buffer(place.buffer).is_some()) {
            Some(Place {
                buffer,
                mut selections,
                scroll,
            }) => {
                self.show(buffer);
                // The buffer may have changed behind `*Messages*`. A cursor that stays where
                // it was keeps its goal, as `C-n` then has it to go on.
                let display = self.current().1.display();
                selections.update(|selection| {
                    let start = motion::cluster_start(&display, selection.cursor);
                    if start != selection.cursor {
                        selection.cursor = start;
                        selection.goal = None;
                    }
                    Ok::<(), ()>(())
                });
                self.restore_positions(selections, scroll);
            }
            None => {
                let scratch = self.scratch();
                self.show(scratch);
            }
        }
    }

    /// Adds a row to `*Messages*`, and takes the oldest one out once it holds
    /// [`MAX_MESSAGES`]. The text has no line break, as [`log_text`] escapes them, so each
    /// message is one row. Cursors at the end stay at the end, and the view follows the
    /// selected window's.
    fn add_to_messages_buffer(&mut self, shown: &str) {
        let id = self.messages_buffer;
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let end = buffer.text().len();
        // The last line is the empty one after the newest message.
        let full = buffer.text().line_count() > MAX_MESSAGES;
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
    use crate::selection::{Goal, Selection};
    use crate::window::ViewSize;
    use std::panic::{AssertUnwindSafe, catch_unwind};
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
        let path = PathBuf::from("/tmp/.env");
        let mut buffer = Buffer::visiting(path.clone());
        buffer.judge_secret(Some(&[path]));
        Editor::with_buffer(buffer)
    }

    #[test]
    fn the_log_shows_control_characters_as_escapes() {
        let shown = log_text("a\u{1b}[2J\u{9b}\u{202e}\n日本語e\u{301}");
        assert_eq!(shown, "a\\u{1b}[2J\\u{9b}\\u{202e}\\n日本語e\u{301}");
    }

    #[test]
    fn the_log_leaves_quotes_and_backslashes_alone() {
        let message = r#"Can't open "a\b.txt" 'x'\"#;
        assert_eq!(log_text(message), message);
        assert_eq!(log_text("'\u{1b}'"), "'\\u{1b}'");
        assert_eq!(log_text(""), "");
    }

    #[test]
    fn the_log_keeps_a_full_width_space_and_escapes_what_can_hide() {
        assert_eq!(log_text("メモ\u{3000}2.txt"), "メモ\u{3000}2.txt");
        assert_eq!(log_text("\u{3000}"), "\u{3000}");
        assert_eq!(
            log_text("a\u{a0}b"),
            "a\\u{a0}b",
            "a no-break space looks like a space"
        );
        assert_eq!(log_text("a\u{3000}\n\u{202e}"), "a\u{3000}\\n\\u{202e}");
    }

    #[test]
    fn a_full_width_space_stays_in_one_row_of_messages() {
        let mut editor = Editor::default();
        editor.message("Wrote メモ\u{3000}2.txt");
        editor.message("Wrote a\u{3000}\nb");
        assert_eq!(
            messages_text(&editor),
            "Wrote メモ\u{3000}2.txt\nWrote a\u{3000}\\nb\n"
        );
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
            echo(&editor),
            "cannot open a\nb\u{202e}c",
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
        assert_eq!(messages_text(&editor), "");
        editor.clear_echo();
        assert_eq!(echo(&editor), "");
    }

    #[test]
    fn what_was_echoed_around_a_secret_goes_for_good_when_the_window_moves_on() {
        let mut editor = on_a_secret();
        let secret = editor.window().buffer();
        editor.echo("Said around it");
        assert_eq!(
            echo(&editor),
            "Said around it",
            "for the person at the screen"
        );
        let other = editor.add_buffer(Buffer::new("other"));
        editor.show(other);
        assert_eq!(echo(&editor), "", "with the secret out of view");
        editor.show(secret);
        assert_eq!(echo(&editor), "", "it does not come back");
    }

    #[test]
    fn what_was_echoed_around_a_secret_goes_with_a_killed_buffer() {
        let mut editor = on_a_secret();
        let secret = editor.window().buffer();
        editor.echo("Said around it");
        editor.kill_buffer(secret);
        assert_eq!(echo(&editor), "");
    }

    #[test]
    fn a_message_from_a_command_around_a_secret_stays_in_the_echo_area() {
        let mut editor = on_a_secret();
        editor.begin_command();
        editor.message("Said around it");
        editor.end_command();
        assert_eq!(echo(&editor), "Said around it");
        assert_eq!(messages_text(&editor), "");
    }

    #[test]
    fn a_message_from_a_command_that_left_a_secret_goes_nowhere() {
        let mut editor = on_a_secret();
        let other = editor.add_buffer(Buffer::new("other"));
        editor.begin_command();
        editor.show(other);
        editor.message("Said around it");
        editor.echo("Said around it");
        editor.end_command();
        assert_eq!(echo(&editor), "");
        assert_eq!(messages_text(&editor), "");
    }

    #[test]
    fn a_message_from_a_command_that_killed_a_secret_goes_nowhere() {
        let mut editor = on_a_secret();
        let secret = editor.window().buffer();
        editor.begin_command();
        editor.kill_buffer(secret);
        editor.message("Said around it");
        editor.end_command();
        assert_eq!(echo(&editor), "");
        assert_eq!(messages_text(&editor), "");
    }

    #[test]
    fn a_message_after_a_command_ends_is_kept_whatever_order_the_group_closes_in() {
        let mut editor = on_a_secret();
        editor.begin_command();
        editor.begin_undo_group();
        editor.end_undo_group();
        editor.message("Said around it");
        assert_eq!(
            messages_text(&editor),
            "",
            "the group's end changes nothing"
        );
        editor.end_command();
        editor.message("Wrote /tmp/.env");
        assert_eq!(messages_text(&editor), "Wrote /tmp/.env\n");
    }

    #[test]
    fn a_message_from_outside_a_command_is_kept_around_a_secret() {
        let mut editor = on_a_secret();
        editor.message("Wrote /tmp/.env");
        assert_eq!(messages_text(&editor), "Wrote /tmp/.env\n");
    }

    #[test]
    fn a_message_and_its_row_are_capped_on_a_character_boundary() {
        let mut editor = Editor::default();
        editor.message("日".repeat(MAX_MESSAGE_BYTES));
        let newest = messages_text(&editor);
        assert!(newest.len() <= MAX_MESSAGE_BYTES + 1, "{}", newest.len());
        assert!(newest.ends_with("…\n"));
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
        assert_eq!(editor.place_behind_internal(), None);
        editor.toggle_messages();
        assert_eq!(editor.current().1.name(), crate::editor::SCRATCH);
    }

    #[test]
    fn the_place_behind_internal_is_what_it_goes_back_to() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", "abcdef"));
        let id = editor.window().buffer();
        editor.current_mut().0.selections = SelectionSet::new(3);
        assert_eq!(editor.place_behind_internal(), None);
        editor.toggle_messages();
        let selections = SelectionSet::new(3);
        assert_eq!(
            editor.place_behind_internal(),
            Some((id, &selections, Scroll::default()))
        );
    }

    #[test]
    fn what_was_said_around_an_unjudged_file_stays_once_it_is_ordinary() {
        let plain = PathBuf::from("/tmp/plain.txt");
        let mut editor = Editor::with_buffer(Buffer::visiting(plain.clone()));
        let id = editor.window().buffer();
        editor.begin_command();
        editor.message("Cannot save a file that is still loading");
        editor.end_command();
        assert_eq!(echo(&editor), "Cannot save a file that is still loading");
        assert_eq!(
            messages_text(&editor),
            "",
            "nothing leaves before it is judged"
        );

        editor.buffer_mut(id).unwrap().judge_secret(Some(&[plain]));
        assert_eq!(echo(&editor), "Cannot save a file that is still loading");
        assert_eq!(
            messages_text(&editor),
            "",
            "it did not reach them at the time"
        );
        editor.clear_echo();
        assert_eq!(echo(&editor), "");
    }

    #[test]
    fn what_was_said_around_an_unjudged_file_is_withheld_once_it_is_secret() {
        let env = PathBuf::from("/tmp/.env");
        let mut editor = Editor::with_buffer(Buffer::visiting(env.clone()));
        let id = editor.window().buffer();
        editor.echo("Cannot save a file that is still loading");
        editor.buffer_mut(id).unwrap().judge_secret(Some(&[env]));
        assert_eq!(echo(&editor), "Cannot save a file that is still loading");
        let other = editor.add_buffer(Buffer::new("other"));
        editor.show(other);
        assert_eq!(echo(&editor), "");
    }

    #[test]
    fn what_was_said_around_an_unjudged_file_goes_when_the_window_moves_on() {
        let mut editor = Editor::with_buffer(Buffer::visiting(PathBuf::from("/tmp/plain.txt")));
        editor.echo("Cannot save a file that is still loading");
        let other = editor.add_buffer(Buffer::new("other"));
        editor.show(other);
        assert_eq!(echo(&editor), "");
    }

    #[test]
    fn a_message_behind_keys_in_progress_waits_for_them_to_end() {
        let mut editor = Editor::default();
        editor.set_keys_in_progress("C-x-");
        editor.message("Not saved");
        assert_eq!(echo(&editor), "C-x-");
        editor.clear_echo();
        editor.echo("Quit");
        editor.set_keys_in_progress("");
        assert_eq!(echo(&editor), "Not saved", "not yet seen");
        editor.clear_echo();
        assert_eq!(echo(&editor), "", "the next key takes it");
    }

    #[test]
    fn going_back_keeps_the_goal_of_a_cursor_that_stays_and_clears_that_of_one_that_moves() {
        let text = "ab\ncde\u{301}f\n";
        let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
        let goal = Some(Goal::Column(9));
        let at = |cursor| Selection { cursor, goal };
        let set = SelectionSet::from_parts(vec![at(1), at(6)], 0).unwrap();
        editor.current_mut().0.selections = set;
        editor.toggle_messages();
        editor.toggle_messages();
        let cursors: Vec<_> = editor.window().selections.iter().copied().collect();
        // Offset 6 is the middle of the cluster that starts at 5, a letter with its mark.
        assert_eq!(
            cursors,
            [
                at(1),
                Selection {
                    cursor: 5,
                    goal: None
                }
            ]
        );
    }

    #[test]
    fn a_panic_adding_a_row_is_not_caught_and_the_echo_area_and_the_row_before_it_stay() {
        let mut editor = Editor::default();
        editor.message("one");
        // Nothing else in a message looks at the selected window.
        editor.selected = usize::MAX;
        let added = catch_unwind(AssertUnwindSafe(|| editor.message("two")));
        assert!(
            added.is_err(),
            "the hook must see an ending, and get one (ADR 0062)"
        );
        editor.selected = 0;
        assert_eq!(echo(&editor), "two");
        assert_eq!(
            messages_text(&editor),
            "one\ntwo\n",
            "the row went in first"
        );
    }

    #[test]
    fn what_a_command_says_after_leaving_a_secret_for_an_unjudged_file_goes_nowhere() {
        let mut editor = on_a_secret();
        let plain = PathBuf::from("/tmp/plain.txt");
        let unjudged = editor.add_buffer(Buffer::visiting(plain.clone()));
        editor.begin_command();
        editor.show(unjudged);
        editor.message("Said around it");
        editor.echo("Said around it");
        editor.end_command();
        editor
            .buffer_mut(unjudged)
            .unwrap()
            .judge_secret(Some(&[plain]));
        assert_eq!(echo(&editor), "");
        assert_eq!(messages_text(&editor), "");
    }

    #[test]
    fn what_a_command_says_after_passing_through_a_secret_goes_nowhere() {
        let mut editor = Editor::with_buffer(Buffer::new("plain"));
        let plain = editor.window().buffer();
        let secret = editor.add_buffer(Buffer::visiting(PathBuf::from("/tmp/.env")));
        editor.begin_command();
        editor.show(secret);
        editor.show(plain);
        editor.message("Said around it");
        editor.end_command();
        assert_eq!(echo(&editor), "");
        assert_eq!(messages_text(&editor), "");
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
