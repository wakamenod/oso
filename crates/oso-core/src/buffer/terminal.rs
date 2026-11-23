//! A terminal's buffer (ADR 0120): a buffer of kind `Terminal`, which holds nothing of the
//! grid. The tip crate maps its id to the terminal; the buffer only says what it is, whether
//! a program runs in it, whether that program reads a password, whether the person let the
//! agent channel read it, and what the modeline says of it.
//!
//! Must not depend on: `window`, `editor` or `command`.
//!
//! It visits no file and holds no text, so it is never saved, carried over a restart, copied
//! in an emergency or counted as unsaved. It is read-only, its mode is `Terminal`, and the
//! agent channel reads it only where the person allowed it (ADR 0120, Q16). While its program
//! reads a password it counts as secret, as a file that may hold secrets does (Q17).

use super::{Buffer, Mode};
use crate::secret::Verdict;

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
        if !running {
            (self.job, self.turn) = (false, false);
        }
    }

    /// Whether work is under way in its terminal (ADR 0133): in a hosted session's, a turn or
    /// a question it asks, as the sessions' worker read its screen; in another, a job its
    /// shell waits for, as the PTY's foreground process group says. A native session's buffer
    /// (ADR 0165) is busy by its turn, as a hosted session's, though the host does not mark it
    /// as hosting a session. Never while nothing runs.
    pub fn is_busy(&self) -> bool {
        let turn = self.session || self.is_native_session();
        self.running && if turn { self.turn } else { self.job }
    }

    pub(crate) fn set_job(&mut self, job: bool) {
        self.job = job;
    }

    pub(crate) fn set_turn(&mut self, turn: bool) {
        self.turn = turn;
    }

    pub(crate) fn set_mode_note(&mut self, note: String) {
        self.mode_note = note;
    }

    /// Whether the person let the agent channel read this terminal's rows, until its buffer
    /// is killed (ADR 0120, Q16). Rows read while its program reads a password are withheld
    /// even so.
    pub fn agent_reads(&self) -> bool {
        self.agent_reads
    }

    pub(crate) fn set_agent_reads(&mut self, reads: bool) {
        self.agent_reads = reads;
    }

    /// Whether its terminal hosts an agent session, as the host said: session mode's keys
    /// apply, the modeline says `Session`, and the buffer is in its session's project
    /// (ADR 0132). It stays so after the session's program ends, for `session-resume`.
    pub fn hosts_session(&self) -> bool {
        self.session
    }

    pub(crate) fn set_session(&mut self, session: bool) {
        self.session = session;
        if !session {
            self.session_directory = None;
        }
    }

    /// The directory its session runs in, as the host gave it for finding its project.
    pub(crate) fn session_directory(&self) -> Option<&std::path::Path> {
        self.session_directory.as_deref()
    }

    pub(crate) fn set_session_directory(&mut self, directory: std::path::PathBuf) {
        self.session_directory = Some(directory);
    }

    /// Counts the terminal as secret while its program reads a password, and as ordinary
    /// once it stops.
    pub(crate) fn set_echo_off(&mut self, echo_off: bool) {
        self.verdict = if echo_off {
            Verdict::EchoOff
        } else {
            Verdict::Ordinary
        };
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

    #[test]
    fn a_password_prompt_makes_it_secret_until_it_ends() {
        let mut buffer = Buffer::terminal("*terminal*");
        assert!(!buffer.is_secret());
        buffer.set_echo_off(true);
        assert!(buffer.is_secret());
        assert_eq!(buffer.verdict(), crate::secret::Verdict::EchoOff);
        assert!(!buffer.verdict().may_be_copied());
        assert!(buffer.verdict().may_be_killed());
        buffer.set_echo_off(false);
        assert!(!buffer.is_secret());
    }

    #[test]
    fn the_agent_reads_only_once_allowed() {
        let mut buffer = Buffer::terminal("*terminal*");
        assert!(!buffer.agent_reads());
        buffer.set_agent_reads(true);
        assert!(buffer.agent_reads());
        // Still withheld from the channel's buffer text: rows come from the host.
        assert!(buffer.withheld_from_agents());
    }
}
