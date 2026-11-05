//! The editor state that commands operate on.
//!
//! Must not depend on: `command`, `keymap`, or the host's window, threads or
//! I/O.
//!
//! The host owns one [`Editor`] and lends it to each command through
//! [`Ctx`](crate::command::Ctx). It holds the buffers and the windows that
//! show them, each window with its own selection set (ADR 0007). There is
//! one window until splits arrive (M3). New state is added here, so
//! commands keep their signature as it grows.
//!
//! Text changes here ([`Editor::apply_edits`], ADR 0053), because a buffer does
//! not know its windows. An edit is made on clones, every window's cursors are
//! worked out against the result, and only then does anything change, in moves
//! that cannot panic. An undo group spans a command: the dispatcher opens one
//! before it runs a command and closes it after, also when the command panics.
//!
//! Saving and the exit guard are in [`save`]: a command starts a save here, and the
//! host, which owns threads and files, takes the job and reports back.

use std::collections::VecDeque;

use crate::buffer::{Buffer, BufferId, EditError, Replacement};
use crate::display_map::DisplaySnapshot;
use crate::motion;
use crate::selection::{OffsetEdit, SelectionSet, new_text_ends};
use crate::sum_tree::Bias;
use crate::undo::{Direction, Opening};
use crate::window::{self, Scroll, ViewSize, Window};

mod save;

pub use save::{ExitAction, ExitRefused, SaveJob, SaveRefused};

/// The name of the buffer shown when no file is open.
pub const SCRATCH: &str = "*scratch*";

/// The messages the editor keeps, as Emacs's `message-log-max`.
const MAX_MESSAGES: usize = 1000;

/// A message as the log shows it: control and non-printable characters are
/// escaped, so that a name in it cannot move a terminal's cursor or reorder the
/// line. Quotes and backslashes stay as they are.
fn log_text(text: &str) -> String {
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

/// What the selected window's cursors do when a buffer it shows changes.
#[derive(Clone, Copy)]
enum Selected<'a> {
    /// It made the edit: a cursor after new text stays after it.
    Edited,
    /// An undo or a redo returns it to these cursors, if the group has them.
    Restored(Option<&'a SelectionSet>),
}

/// Moves every cursor where a cursor can be: to the start of the display cluster
/// around it, or to the end if it is at one of `ends` (ascending), just after new
/// text. New text can join what follows it, and what was typed must stay before
/// the cursor. Goals are cleared.
fn clip_cursors(set: &mut SelectionSet, display: &DisplaySnapshot, ends: &[usize]) {
    let mut ends = ends.iter().copied().peekable();
    set.update(|selection| {
        while ends.next_if(|end| *end < selection.cursor).is_some() {}
        selection.cursor = if ends.peek() == Some(&selection.cursor) {
            motion::cluster_end(display, selection.cursor)
        } else {
            motion::cluster_start(display, selection.cursor)
        };
        selection.goal = None;
        Ok::<(), ()>(())
    });
}

/// The undo group of the command running.
struct UndoScope {
    /// The buffer the selected window showed when it began, with the cursors.
    before: (BufferId, SelectionSet),
    /// The buffers this command edited, each with its newest group open.
    touched: Vec<BufferId>,
    /// The command allowed its edits to join the previous command's group.
    join: bool,
    /// The command undid or redid something.
    crossed: bool,
}

/// All editor state that commands read and change.
///
/// Invariant: every window shows a buffer that exists.
pub struct Editor {
    buffers: Vec<(BufferId, Buffer)>,
    windows: Vec<Window>,
    selected: usize,
    next_id: u64,
    undo_scope: Option<UndoScope>,
    /// The previous command allowed the next one to join its undo group.
    join_ready: bool,
    /// Oldest first.
    messages: VecDeque<String>,
    /// Saves the host has yet to start.
    save_jobs: Vec<SaveJob>,
    /// The way of exiting that was last refused, until another key is pressed.
    exit_attempt: Option<ExitAction>,
}

impl Default for Editor {
    /// An editor showing an empty `*scratch*` buffer.
    fn default() -> Self {
        Self::with_buffer(Buffer::new(SCRATCH))
    }
}

impl Editor {
    /// An editor whose one window shows `buffer`.
    pub fn with_buffer(buffer: Buffer) -> Self {
        let mut editor = Self {
            buffers: Vec::new(),
            windows: Vec::new(),
            selected: 0,
            next_id: 0,
            undo_scope: None,
            join_ready: false,
            messages: VecDeque::new(),
            save_jobs: Vec::new(),
            exit_attempt: None,
        };
        let id = editor.add_buffer(buffer);
        editor.windows.push(Window::new(id));
        editor
    }

    /// Adds a buffer, shown in no window yet.
    pub fn add_buffer(&mut self, buffer: Buffer) -> BufferId {
        let id = BufferId(self.next_id);
        self.next_id += 1;
        self.buffers.push((id, buffer));
        id
    }

    /// The buffer with `id`, if it exists.
    pub fn buffer(&self, id: BufferId) -> Option<&Buffer> {
        self.buffers
            .iter()
            .find_map(|(other, buffer)| (*other == id).then_some(buffer))
    }

    /// The buffer with `id`, if it exists, to change.
    pub fn buffer_mut(&mut self, id: BufferId) -> Option<&mut Buffer> {
        self.buffers
            .iter_mut()
            .find_map(|(other, buffer)| (*other == id).then_some(buffer))
    }

    /// Every buffer, in the order they were added.
    pub fn buffers(&self) -> impl Iterator<Item = (BufferId, &Buffer)> {
        self.buffers.iter().map(|(id, buffer)| (*id, buffer))
    }

    /// Shows `buffer` in the selected window, from its start. Does nothing
    /// for an id that does not exist.
    pub fn show(&mut self, buffer: BufferId) {
        if self.buffer(buffer).is_some() {
            self.windows[self.selected].show(buffer);
        }
    }

    /// Removes a buffer. Windows that showed it show `*scratch*` instead,
    /// which is created again if needed.
    pub fn kill_buffer(&mut self, id: BufferId) {
        self.buffers.retain(|(other, _)| *other != id);
        if self.windows.iter().all(|window| window.buffer() != id) {
            return;
        }
        let scratch = match self
            .buffers
            .iter()
            .find(|(_, buffer)| buffer.name() == SCRATCH && buffer.path().is_none())
        {
            Some((scratch, _)) => *scratch,
            None => self.add_buffer(Buffer::new(SCRATCH)),
        };
        for window in &mut self.windows {
            if window.buffer() == id {
                window.show(scratch);
            }
        }
    }

    /// Every window.
    pub fn windows(&self) -> impl ExactSizeIterator<Item = &Window> {
        self.windows.iter()
    }

    /// The position of the selected window in [`Editor::windows`].
    pub fn selected_window(&self) -> usize {
        self.selected
    }

    /// Whether any window shows a buffer that may hold secrets (ADR 0044).
    pub fn shows_secret(&self) -> bool {
        self.windows
            .iter()
            .any(|window| self.shown(window).is_secret())
    }

    /// The selected window.
    pub fn window(&self) -> &Window {
        &self.windows[self.selected]
    }

    /// The selected window and the buffer it shows.
    pub fn current(&self) -> (&Window, &Buffer) {
        let window = &self.windows[self.selected];
        (window, self.shown(window))
    }

    /// The selected window, to change, and the buffer it shows.
    #[expect(
        clippy::expect_used,
        reason = "every window shows a buffer that exists: kill_buffer moves windows off a buffer before it goes"
    )]
    pub fn current_mut(&mut self) -> (&mut Window, &Buffer) {
        let window = &mut self.windows[self.selected];
        let id = window.buffer();
        let buffer = self
            .buffers
            .iter()
            .find_map(|(other, buffer)| (*other == id).then_some(buffer))
            .expect("every window shows a buffer that exists");
        (window, buffer)
    }

    /// The buffer `window` shows.
    #[expect(
        clippy::expect_used,
        reason = "every window shows a buffer that exists: kill_buffer moves windows off a buffer before it goes"
    )]
    pub fn shown(&self, window: &Window) -> &Buffer {
        self.buffer(window.buffer())
            .expect("every window shows a buffer that exists")
    }

    /// Applies `edits` to the buffer `id` as one step, and moves the cursors of
    /// every window showing it. All of it happens, or none of it does.
    ///
    /// `edits` are usually one per cursor, in any order. They are applied in
    /// descending offset order, so all their ranges are offsets in the text as
    /// it is now. Ranges that overlap, and insertions at one offset, become
    /// their union, replaced by their texts joined in offset order; ranges that
    /// only touch stay apart. Empty edits do nothing.
    ///
    /// In the selected window, a cursor at the start of an edit or inside its
    /// replaced text ends after the new text, so a command that inserts at each
    /// cursor need not move them, though it may move them afterward. In other
    /// windows such a cursor stays before the new text, as an Emacs marker.
    /// Cursors are then clipped to a display cluster, since a combining mark can
    /// move the boundaries: to its start, or to its end for a selected cursor
    /// just after new text that joined what follows, so typed text stays before
    /// the cursor. Cursors that met merge.
    ///
    /// # Errors
    ///
    /// [`EditError::ReadOnly`] for a read-only buffer, and
    /// [`EditError::NoSuchBuffer`]. Nothing changes.
    pub fn apply_edits(
        &mut self,
        id: BufferId,
        edits: &[Replacement<'_>],
    ) -> Result<(), EditError> {
        let buffer = self.buffer(id).ok_or(EditError::NoSuchBuffer)?;
        let Some(staged) = buffer.stage_edits(edits)? else {
            return Ok(());
        };
        let opening = self.opening(id, buffer);
        let moved = self.moved_selections(id, &staged.display(), staged.steps(), Selected::Edited);
        let after = moved
            .iter()
            .find_map(|(index, set)| (*index == self.selected).then(|| set.clone()));

        // Nothing past this point can panic part way.
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.commit_edit(staged, opening, after);
        }
        for (index, set) in moved {
            self.windows[index].selections = set;
        }
        match &mut self.undo_scope {
            Some(scope) if !scope.touched.contains(&id) => scope.touched.push(id),
            Some(_) => {}
            None => self.join_ready = false,
        }
        Ok(())
    }

    /// How an edit of `id` relates to its newest undo group.
    fn opening(&self, id: BufferId, buffer: &Buffer) -> Opening {
        let Some(scope) = &self.undo_scope else {
            return Opening::New(self.selected_selections(id).cloned());
        };
        if scope.touched.contains(&id) {
            return Opening::Continue;
        }
        let before = if scope.before.0 == id {
            Some(&scope.before.1)
        } else {
            self.selected_selections(id)
        };
        if scope.join && self.join_ready && buffer.can_join_undo(before) {
            Opening::Join
        } else {
            Opening::New(before.cloned())
        }
    }

    /// The selected window's cursors, if it shows `id`.
    fn selected_selections(&self, id: BufferId) -> Option<&SelectionSet> {
        let window = &self.windows[self.selected];
        (window.buffer() == id).then_some(&window.selections)
    }

    /// The cursors of every window showing `id` once a change is in, by window
    /// position. They follow `steps`, as markers except in the selected window,
    /// which does as `selected` says. Each ends where a cursor can be.
    fn moved_selections(
        &self,
        id: BufferId,
        display: &DisplaySnapshot,
        steps: &[Vec<OffsetEdit>],
        selected: Selected<'_>,
    ) -> Vec<(usize, SelectionSet)> {
        let mut moved = Vec::new();
        for (index, window) in self.windows.iter().enumerate() {
            if window.buffer() != id {
                continue;
            }
            let selected = (index == self.selected).then_some(selected);
            let mut set = match selected {
                Some(Selected::Restored(Some(restored))) => restored.clone(),
                _ => window.selections.clone(),
            };
            match selected {
                Some(Selected::Restored(Some(_))) => clip_cursors(&mut set, display, &[]),
                Some(Selected::Edited) => {
                    let mut ends = Vec::new();
                    for edits in steps {
                        set.follow_edits(edits, Bias::Right);
                        ends.extend(new_text_ends(edits));
                    }
                    clip_cursors(&mut set, display, &ends);
                }
                _ => {
                    for edits in steps {
                        set.follow_edits(edits, Bias::Left);
                    }
                    clip_cursors(&mut set, display, &[]);
                }
            }
            moved.push((index, set));
        }
        moved
    }

    /// Opens the undo group of a command. The dispatcher calls it before it
    /// runs a command. Edits until [`Editor::end_undo_group`] undo as one step,
    /// which returns to the selected window's cursors as they are now. A group
    /// left open closes here first, so a missed end costs one step, not all
    /// later ones in a single group.
    pub fn begin_undo_group(&mut self) {
        self.close_undo_scope(false);
        let window = &self.windows[self.selected];
        self.undo_scope = Some(UndoScope {
            before: (window.buffer(), window.selections.clone()),
            touched: Vec::new(),
            join: false,
            crossed: false,
        });
    }

    /// Lets the edits of the running command join the previous command's undo
    /// group, if that command called this too, edited something and undid
    /// nothing, and the cursors are where it left them. For self-inserted characters, up to a limit (ADR 0030). Does
    /// nothing with no group open.
    pub fn allow_undo_join(&mut self) {
        if let Some(scope) = &mut self.undo_scope {
            scope.join = true;
        }
    }

    /// Closes the undo group [`Editor::begin_undo_group`] opened. The group
    /// ends with the selected window's cursors as they are now, so a command may
    /// move them after its edits. Does nothing with no group open.
    pub fn end_undo_group(&mut self) {
        self.close_undo_scope(true);
    }

    /// Closes the open undo group when its command panicked. What the command
    /// already applied stays in the buffer as one undo step, and nothing joins
    /// it. It never panics, and does nothing with no group open.
    pub fn end_undo_group_after_panic(&mut self) {
        self.close_undo_scope(false);
    }

    fn close_undo_scope(&mut self, may_join: bool) {
        let Some(scope) = self.undo_scope.take() else {
            return;
        };
        self.join_ready = may_join && scope.join && !scope.crossed && !scope.touched.is_empty();
        for id in scope.touched {
            self.seal_undo_group(id);
        }
    }

    /// Records the selected window's cursors as the end of the newest undo
    /// group of `id`, if it shows that buffer.
    fn seal_undo_group(&mut self, id: BufferId) {
        if let Some(selections) = self.selected_selections(id).cloned()
            && let Some(buffer) = self.buffer_mut(id)
        {
            buffer.set_undo_after(selections);
        }
    }

    /// Undoes the newest undo group of the selected window's buffer. The window
    /// gets the cursors from before the group, and other windows' cursors follow
    /// the text. The buffer's content id returns to the one before the group.
    ///
    /// # Errors
    ///
    /// [`EditError::NoUndo`] and [`EditError::ReadOnly`]. Nothing changes.
    pub fn undo(&mut self) -> Result<(), EditError> {
        self.cross(Direction::Undo)
    }

    /// Redoes the group the last undo crossed, if no edit came since (ADR
    /// 0030). The selected window gets the cursors from after the group.
    ///
    /// # Errors
    ///
    /// [`EditError::NoRedo`] and [`EditError::ReadOnly`]. Nothing changes.
    pub fn redo(&mut self) -> Result<(), EditError> {
        self.cross(Direction::Redo)
    }

    fn cross(&mut self, direction: Direction) -> Result<(), EditError> {
        let id = self.windows[self.selected].buffer();
        let buffer = self.buffer(id).ok_or(EditError::NoSuchBuffer)?;
        buffer.next_crossing(direction)?;
        // Edits of this command come first in the history, as their own step.
        if let Some(scope) = &mut self.undo_scope
            && let Some(position) = scope.touched.iter().position(|other| *other == id)
        {
            scope.touched.swap_remove(position);
            self.seal_undo_group(id);
        }
        let buffer = self.buffer(id).ok_or(EditError::NoSuchBuffer)?;
        let staged = buffer.stage_crossing(direction)?;
        let selected = Selected::Restored(staged.selections());
        let moved = self.moved_selections(id, &staged.display(), staged.steps(), selected);

        // Nothing past this point can panic part way.
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.commit_crossing(staged);
        }
        for (index, set) in moved {
            self.windows[index].selections = set;
        }
        if let Some(scope) = &mut self.undo_scope {
            scope.before = (id, self.windows[self.selected].selections.clone());
            scope.crossed = true;
        }
        self.join_ready = false;
        Ok(())
    }

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

    /// Sets the size of the windows in rows and columns shown in full, and
    /// scrolls to keep each cursor in view. Returns whether a view moved.
    pub fn set_view_size(&mut self, size: ViewSize) -> bool {
        for window in &mut self.windows {
            window.set_size(size);
        }
        self.follow_cursor()
    }

    /// Scrolls the selected window by the fewest rows and columns that show
    /// its primary cursor (`scroll-conservatively` 101). The host calls it
    /// after every command. Returns whether the view moved.
    pub fn follow_cursor(&mut self) -> bool {
        let (window, buffer) = self.current_mut();
        let display = buffer.display();
        let point = window.cursor_point(&display);
        let cells = motion::cursor_cells(&display, point);
        let scroll = window::follow(window.scroll(), window.size(), point, cells);
        let moved = scroll != window.scroll();
        window.set_scroll(scroll);
        moved
    }

    /// Puts back the selected window's cursors and view, saved together as a
    /// state handoff does, then scrolls as little as shows its primary cursor.
    pub fn restore_positions(&mut self, selections: SelectionSet, scroll: Scroll) {
        let (window, buffer) = self.current_mut();
        let display = buffer.display();
        window.restore_positions(selections, scroll, &display);
        self.follow_cursor();
    }

    /// Scrolls the selected window's view by `rows` (down toward the end
    /// if positive), for the mouse wheel and trackpad. Cursors left outside
    /// the view move to its nearest row, as in Emacs. Returns whether the
    /// view moved.
    pub fn scroll_view(&mut self, rows: isize) -> bool {
        let (window, buffer) = self.current_mut();
        let display = buffer.display();
        let row_count = display.row_count();
        let top = window::wheel(window.scroll().top, window.size(), row_count, rows);
        window.set_top(top, &display)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn lines(n: usize) -> String {
        (0..n).map(|i| format!("line {i:03}\n")).collect()
    }

    #[test]
    fn restoring_positions_follows_the_cursor() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        editor.set_view_size(ViewSize::new(10, 20));
        let saved = Scroll { top: 40, left: 1 };
        editor.restore_positions(SelectionSet::new(45 * "line 000\n".len() + 1), saved);
        assert_eq!(editor.window().scroll(), saved, "the cursor is in view");
        editor.restore_positions(SelectionSet::new(0), saved);
        assert_eq!(editor.window().scroll(), Scroll::default(), "back to it");
        assert_eq!(editor.window().selections.primary().cursor, 0);
    }

    #[test]
    fn starts_with_scratch() {
        let editor = Editor::default();
        let (window, buffer) = editor.current();
        assert_eq!(buffer.name(), SCRATCH);
        assert_eq!(window.selections.primary().cursor, 0);
    }

    #[test]
    fn killing_a_shown_buffer_shows_scratch() {
        let mut editor = Editor::default();
        let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a")));
        editor.show(file);
        assert_eq!(editor.current().1.name(), "a");
        editor.kill_buffer(file);
        assert_eq!(editor.current().1.name(), SCRATCH);
        assert_eq!(editor.buffers().count(), 1);
        // Killing scratch itself makes a new one.
        let (scratch, _) = editor.buffers().next().unwrap();
        editor.kill_buffer(scratch);
        assert_eq!(editor.current().1.name(), SCRATCH);
    }

    #[test]
    fn a_file_named_like_scratch_is_not_scratch() {
        let mut editor = Editor::default();
        let (scratch, _) = editor.buffers().next().unwrap();
        let other = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/b")));
        editor.show(other);
        editor.kill_buffer(scratch);
        let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/*scratch*")));
        editor.kill_buffer(other);
        assert_ne!(editor.current().0.buffer(), file);
        assert_eq!(editor.current().1.path(), None);
    }

    #[test]
    fn buffers_can_be_changed_by_id() {
        let mut editor = Editor::default();
        let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/b")));
        editor.buffer_mut(file).unwrap().finish_empty();
        assert_eq!(editor.buffer(file).unwrap().loading(), None);
        assert_eq!(editor.buffer(file).unwrap().name(), "b");
        assert!(editor.buffer_mut(BufferId(u64::MAX)).is_none());
        assert_eq!(editor.selected_window(), 0);
    }

    #[test]
    fn shows_secret_follows_what_the_windows_show() {
        let mut editor = Editor::default();
        assert!(!editor.shows_secret());
        let file = editor.add_buffer(Buffer::visiting(PathBuf::from("/a/.env")));
        assert!(!editor.shows_secret(), "not shown yet");
        editor.show(file);
        assert!(editor.shows_secret());
        let (scratch, _) = editor.buffers().next().unwrap();
        editor.show(scratch);
        assert!(!editor.shows_secret(), "switched away");
    }

    #[test]
    fn the_view_follows_the_cursor_and_the_wheel_moves_the_cursor() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        editor.set_view_size(ViewSize::new(10, 20));
        editor.windows[0].selections = crate::selection::SelectionSet::new(15 * 9);
        assert!(editor.follow_cursor());
        assert_eq!(editor.window().scroll().top, 6);
        assert!(!editor.follow_cursor());

        assert!(editor.scroll_view(20));
        assert_eq!(editor.window().scroll().top, 26);
        let cursor = editor.window().selections.primary().cursor;
        assert_eq!(cursor, 26 * 9, "moved to the start of the top row");
        assert!(editor.scroll_view(-3));
        assert_eq!(editor.window().selections.primary().cursor, 26 * 9);
        assert!(editor.scroll_view(-100));
        assert_eq!(editor.window().scroll().top, 0);
        assert!(!editor.scroll_view(-1));
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
    fn messages_are_kept_in_order_up_to_a_bound() {
        let mut editor = Editor::default();
        assert_eq!(editor.messages().count(), 0);
        editor.message("Saved");
        editor.message(String::from("Quit"));
        assert_eq!(editor.messages().collect::<Vec<_>>(), ["Saved", "Quit"]);
        assert_eq!(editor.messages().next_back(), Some("Quit"));
        for index in 0..MAX_MESSAGES {
            editor.message(format!("{index}"));
        }
        assert_eq!(editor.messages().len(), MAX_MESSAGES);
        assert_eq!(editor.messages().next(), Some("0"));
        assert_eq!(editor.messages().next_back(), Some("999"));
    }
}

#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod save_tests;
#[cfg(test)]
mod undo_tests;
