//! A terminal's buffer (ADR 0120): a buffer of kind `Terminal`, which holds nothing of the
//! grid. The tip crate maps its id to the terminal; the buffer only says what it is, whether
//! a program runs in it, and what the modeline says of it.
//!
//! Must not depend on: `window`, `editor` or `command`.
//!
//! It visits no file and holds no text, so it is never saved, carried over a restart, copied
//! in an emergency or counted as unsaved. It is read-only, its mode is `Terminal`, and the
//! agent channel may not read it (ADR 0120, Q16).

use super::{Buffer, Mode};

impl Buffer {
    /// An empty terminal's buffer called `name`.
    pub(crate) fn terminal(name: impl Into<String>) -> Self {
        Self {
            terminal: true,
            mode: Mode::Terminal,
            withheld_from_agents: true,
            ..Self::new(name)
        }
    }

    /// Whether this is a terminal's buffer.
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Whether a program runs in its terminal, as the host last said. Killing it asks while
    /// one does.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// What the modeline says after the mode, as the host set it: a terminal's title, its
    /// input mode, or how its program ended. Empty for most buffers.
    pub fn mode_note(&self) -> &str {
        &self.mode_note
    }

    pub(crate) fn set_running(&mut self, running: bool) {
        self.running = running;
    }

    pub(crate) fn set_mode_note(&mut self, note: String) {
        self.mode_note = note;
    }
}

#[cfg(test)]
mod tests {
    use super::{Buffer, Mode};
    use crate::buffer::ReadOnly;

    #[test]
    fn a_terminals_buffer_is_read_only_withheld_and_keeps_its_mode() {
        let mut buffer = Buffer::terminal("*terminal*");
        assert!(buffer.is_terminal());
        assert!(!buffer.is_internal());
        assert_eq!(buffer.mode(), Mode::Terminal);
        assert_eq!(buffer.read_only(), Some(ReadOnly::Terminal));
        assert!(buffer.withheld_from_agents());
        assert!(!buffer.is_modified());
        assert!(!buffer.set_mode(Mode::Rust));
        assert_eq!(buffer.mode(), Mode::Terminal);
    }

    #[test]
    fn no_other_buffer_takes_the_terminal_mode() {
        let mut buffer = Buffer::new("*scratch*");
        assert!(!buffer.set_mode(Mode::Terminal));
        assert!(!buffer.is_terminal());
    }

    #[test]
    fn the_host_says_whether_a_program_runs_and_what_the_modeline_notes() {
        let mut buffer = Buffer::terminal("*terminal*");
        assert!(!buffer.is_running());
        buffer.set_running(true);
        assert!(buffer.is_running());
        assert_eq!(buffer.mode_note(), "");
        buffer.set_mode_note("vim".to_owned());
        assert_eq!(buffer.mode_note(), "vim");
    }
}
