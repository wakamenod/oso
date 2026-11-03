//! The editor state that commands operate on.
//!
//! The host owns one [`Editor`] and lends it to each command through
//! [`Ctx`](crate::command::Ctx). It holds the buffers and the windows that
//! show them, each window with its own selection set (ADR 0007). There is
//! one window until splits arrive (M3). New state is added here, so
//! commands keep their signature as it grows.

use crate::buffer::{Buffer, BufferId};
use crate::motion;
use crate::selection::Selection;
use crate::window::{self, ViewSize, Window};

/// The name of the buffer shown when no file is open.
pub const SCRATCH: &str = "*scratch*";

/// All editor state that commands read and change.
///
/// Invariant: every window shows a buffer that exists.
pub struct Editor {
    buffers: Vec<(BufferId, Buffer)>,
    windows: Vec<Window>,
    selected: usize,
    next_id: u64,
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

    fn shown(&self, window: &Window) -> &Buffer {
        self.buffer(window.buffer())
            .expect("every window shows a buffer that exists")
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
        let point = display.offset_to_display_point(window.selections.primary().cursor);
        let cells = motion::cursor_cells(&display, point);
        let scroll = window::follow(window.scroll, window.size(), point, cells);
        let moved = scroll != window.scroll;
        window.scroll = scroll;
        moved
    }

    /// Scrolls the selected window's view by `rows` (down toward the end
    /// if positive), for the mouse wheel and trackpad. Cursors left outside
    /// the view move to its nearest row, as in Emacs. Returns whether the
    /// view moved.
    pub fn scroll_view(&mut self, rows: isize) -> bool {
        let (window, buffer) = self.current_mut();
        let display = buffer.display();
        let row_count = display.row_count();
        let top = window::wheel(window.scroll.top, window.size(), row_count, rows);
        if top == window.scroll.top {
            return false;
        }
        window.scroll.top = top;
        let size = window.size();
        window.selections.update(|selection| {
            let row = display.offset_to_display_point(selection.cursor).row;
            if let Some(row) = window::row_in_view(row, top, size, row_count) {
                *selection = Selection::at(motion::line_start(&display, row));
            }
            Ok::<(), ()>(())
        });
        true
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
    fn starts_with_scratch() {
        let editor = Editor::default();
        let (window, buffer) = editor.current();
        assert_eq!(buffer.name(), SCRATCH);
        assert_eq!(window.selections.primary().cursor, 0);
    }

    #[test]
    fn killing_a_shown_buffer_shows_scratch() {
        let mut editor = Editor::default();
        let file = editor.add_buffer(Buffer::visiting("a", PathBuf::from("/a")));
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
    fn the_view_follows_the_cursor_and_the_wheel_moves_the_cursor() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", &lines(100)));
        editor.set_view_size(ViewSize::new(10, 20));
        editor.windows[0].selections = crate::selection::SelectionSet::new(15 * 9);
        assert!(editor.follow_cursor());
        assert_eq!(editor.window().scroll.top, 6);
        assert!(!editor.follow_cursor());

        assert!(editor.scroll_view(20));
        assert_eq!(editor.window().scroll.top, 26);
        let cursor = editor.window().selections.primary().cursor;
        assert_eq!(cursor, 26 * 9, "moved to the start of the top row");
        assert!(editor.scroll_view(-3));
        assert_eq!(editor.window().selections.primary().cursor, 26 * 9);
        assert!(!editor.scroll_view(-100) || editor.window().scroll.top == 0);
        assert!(!editor.scroll_view(-1));
    }
}
