//! Terminals' buffers and what commands ask of the host about terminals (ADR 0120).
//!
//! Must not depend on: the host's terminals, threads or I/O. The tip crate maps a terminal
//! buffer's id to its terminal; a command asks, and the host takes the request after the
//! command and acts on the terminal. Nothing here holds a terminal's text.

use crate::buffer::{Buffer, BufferId};
use crate::key::KeyPress;

use super::Editor;

/// What a command asks of the host about the selected terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalRequest {
    /// `terminal`: switch to the last terminal still running, or start one; with `new`,
    /// always start one.
    Open { new: bool },
    /// Send these keys to the selected terminal's program, encoded on its thread.
    Keys(Vec<KeyPress>),
    /// Paste this text into the selected terminal's program, as `C-y` does.
    Paste { text: String },
    /// The person said yes to pasting lines without bracketed paste.
    PasteLines,
    /// Send the next key to the program as it is, oso's keys among them (`C-q`).
    QuoteNext,
    /// Switch the selected terminal between semi-char and char mode (`C-c C-k`).
    ToggleCharMode,
}

impl Editor {
    /// Adds a terminal's buffer, shown in no window yet, named `base`, or `base<2>`,
    /// `base<3>` and on when that is taken. It visits no file, holds no text, is never saved,
    /// carried or counted at an exit, and is withheld from agents.
    pub fn add_terminal_buffer(&mut self, base: &str) -> BufferId {
        let taken = |name: &str| self.buffers().any(|(_, buffer)| buffer.name() == name);
        let mut name = base.to_owned();
        let mut number = 2_u64;
        while taken(&name) {
            name = format!("{base}<{number}>");
            number += 1;
        }
        self.add_buffer(Buffer::terminal(name))
    }

    /// Notes whether a program runs in the terminal buffer `id`: killing it asks while one
    /// does, and so does an exit. Nothing happens to a buffer that is not a terminal's.
    pub fn set_running(&mut self, id: BufferId, running: bool) {
        if let Some(buffer) = self.buffer_mut(id).filter(|buffer| buffer.is_terminal()) {
            buffer.set_running(running);
        }
    }

    /// The terminal buffers a program runs in, in the order they were made.
    pub fn running_terminals(&self) -> impl Iterator<Item = BufferId> + '_ {
        self.buffers()
            .filter(|(_, buffer)| buffer.is_terminal() && buffer.is_running())
            .map(|(id, _)| id)
    }

    /// Sets what the modeline of `id` says after its mode, such as a terminal's title. The
    /// modeline cuts it to fit. The host cleans it first: a terminal's title has no control,
    /// bidi or invisible characters, and is at most 256 bytes (ADR 0120).
    pub fn set_mode_note(&mut self, id: BufferId, note: String) {
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_mode_note(note);
        }
    }

    /// A command asks the host for `request`.
    pub fn request_terminal(&mut self, request: TerminalRequest) {
        self.terminal_requests.push(request);
    }

    /// What commands asked about terminals since the host last looked, oldest first.
    pub fn take_terminal_requests(&mut self) -> Vec<TerminalRequest> {
        std::mem::take(&mut self.terminal_requests)
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalRequest;
    use crate::buffer::{Mode, ReadOnly};
    use crate::editor::{Editor, KillRefused};

    #[test]
    fn terminals_are_named_apart() {
        let mut editor = Editor::default();
        let first = editor.add_terminal_buffer("*terminal*");
        let second = editor.add_terminal_buffer("*terminal*");
        let third = editor.add_terminal_buffer("*terminal*");
        let name = |id| editor.buffer(id).map(|b| b.name().to_owned()).unwrap();
        assert_eq!(name(first), "*terminal*");
        assert_eq!(name(second), "*terminal*<2>");
        assert_eq!(name(third), "*terminal*<3>");
        let buffer = editor.buffer(first).unwrap();
        assert_eq!(buffer.mode(), Mode::Terminal);
        assert_eq!(buffer.read_only(), Some(ReadOnly::Terminal));
    }

    #[test]
    fn running_is_a_terminals_and_killing_asks_while_one_runs() {
        let mut editor = Editor::default();
        let terminal = editor.add_terminal_buffer("*terminal*");
        let scratch = editor.window().buffer();
        editor.set_running(scratch, true);
        assert!(!editor.buffer(scratch).unwrap().is_running());
        editor.set_running(terminal, true);
        assert_eq!(editor.running_terminals().collect::<Vec<_>>(), [terminal]);
        assert_eq!(
            editor.kill_by_person(terminal, false),
            Err(KillRefused::Running)
        );
        assert_eq!(editor.kill_asked(), Some(terminal));
        assert_eq!(editor.kill_by_person(terminal, true), Ok(()));
        assert!(editor.buffer(terminal).is_none());
    }

    #[test]
    fn an_ended_terminal_is_killed_without_asking() {
        let mut editor = Editor::default();
        let terminal = editor.add_terminal_buffer("*terminal*");
        editor.set_running(terminal, true);
        editor.set_running(terminal, false);
        assert_eq!(editor.running_terminals().count(), 0);
        assert_eq!(editor.kill_by_person(terminal, false), Ok(()));
    }

    #[test]
    fn a_terminal_does_not_hold_an_exit_as_unsaved_text() {
        let mut editor = Editor::default();
        let terminal = editor.add_terminal_buffer("*terminal*");
        editor.set_running(terminal, true);
        assert_eq!(editor.confirm_exit(false), Ok(()));
    }

    #[test]
    fn the_note_is_the_buffers() {
        let mut editor = Editor::default();
        let terminal = editor.add_terminal_buffer("*terminal*");
        editor.set_mode_note(terminal, "htop".to_owned());
        assert_eq!(editor.buffer(terminal).unwrap().mode_note(), "htop");
    }

    #[test]
    fn requests_are_taken_once_in_order() {
        let mut editor = Editor::default();
        editor.request_terminal(TerminalRequest::Open { new: false });
        editor.request_terminal(TerminalRequest::ToggleCharMode);
        assert_eq!(
            editor.take_terminal_requests(),
            [
                TerminalRequest::Open { new: false },
                TerminalRequest::ToggleCharMode
            ]
        );
        assert!(editor.take_terminal_requests().is_empty());
    }
}
