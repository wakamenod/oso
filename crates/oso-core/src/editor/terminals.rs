//! Terminals' buffers and what commands ask of the host about terminals (ADR 0120), and the
//! restart that carries them to the binary installed in place (ADR 0133).
//!
//! Must not depend on: the host's terminals, threads or I/O. The tip crate maps a terminal
//! buffer's id to its terminal; a command asks, and the host takes the request after the
//! command and acts on the terminal. Nothing here holds a terminal's text.

use crate::buffer::{Buffer, BufferId};
use crate::key::KeyPress;

use super::Editor;
use super::sessions::takes_session_flags;

/// What a command asks of the host about the selected terminal, or about every terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalRequest {
    /// `terminal`: switch to the last shell terminal still running, never a session's, or
    /// start one; with `new`, always start one.
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
    /// `oso-restart`: move this instance, its terminals and sessions with it, to the binary
    /// installed in its place (ADR 0133).
    Restart,
}

impl Editor {
    /// Adds a terminal's buffer, shown in no window yet, named `base`, or `base<2>`,
    /// `base<3>` and on when that is taken. It visits no file, holds no text, is never saved,
    /// carried or counted at an exit, and is withheld from agents.
    pub fn add_terminal_buffer(&mut self, base: &str) -> BufferId {
        let name = self.free_terminal_name(base, None);
        self.add_buffer(Buffer::terminal(name))
    }

    /// `base`, or `base<2>`, `base<3>` and on, the first no buffer but `own` has: the names
    /// that start with `base` gathered in one pass, so a run of taken numbers costs one look
    /// at each buffer.
    fn free_terminal_name(&self, base: &str, own: Option<BufferId>) -> String {
        let taken: std::collections::HashSet<&str> = (self.buffers())
            .filter(|(id, buffer)| Some(*id) != own && buffer.name().starts_with(base))
            .map(|(_, buffer)| buffer.name())
            .collect();
        let mut name = base.to_owned();
        let mut number = 2_u64;
        while taken.contains(name.as_str()) {
            name = format!("{base}<{number}>");
            number += 1;
        }
        name
    }

    /// Notes whether a program runs in the terminal buffer `id`, or a native session's
    /// (ADR 0165): killing it asks while one does, and so does an exit. Nothing happens to
    /// another buffer.
    pub fn set_running(&mut self, id: BufferId, running: bool) {
        if let Some(buffer) = self
            .buffer_mut(id)
            .filter(|buffer| takes_session_flags(buffer))
        {
            buffer.set_running(running);
        }
    }

    /// Notes whether a job runs in the foreground of the terminal buffer `id`'s PTY, which
    /// makes a terminal that hosts no session busy (ADR 0133). Nothing happens to a buffer that
    /// is not a terminal's.
    pub fn set_terminal_job(&mut self, id: BufferId, job: bool) {
        if let Some(buffer) = self.buffer_mut(id).filter(|buffer| buffer.is_terminal()) {
            buffer.set_job(job);
        }
    }

    /// Notes whether the session hosted in the terminal buffer `id`, or shown in a native
    /// session's, has a turn under way or asks a question, which makes it busy (ADR 0133).
    /// Nothing happens to another buffer.
    pub fn set_session_turn(&mut self, id: BufferId, turn: bool) {
        if let Some(buffer) = self
            .buffer_mut(id)
            .filter(|buffer| takes_session_flags(buffer))
        {
            buffer.set_turn(turn);
        }
    }

    /// The terminal buffers work is under way in, in the order they were made.
    pub fn busy_terminals(&self) -> impl Iterator<Item = BufferId> + '_ {
        self.buffers()
            .filter(|(_, buffer)| buffer.is_terminal() && buffer.is_busy())
            .map(|(id, _)| id)
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

    /// Notes whether the program in the terminal buffer `id` reads a password (ADR 0120,
    /// Q17): while it does, the buffer counts as secret, so the agent channel waits while it
    /// is shown and secure input is on while it is selected. Nothing happens to a buffer that
    /// is not a terminal's.
    pub fn set_echo_off(&mut self, id: BufferId, echo_off: bool) {
        if let Some(buffer) = self.buffer_mut(id).filter(|buffer| buffer.is_terminal()) {
            buffer.set_echo_off(echo_off);
        }
    }

    /// Lets the agent channel read the terminal buffer `id`'s rows, or a native session's
    /// text, or takes it back (ADR 0120, Q16; ADR 0165). Nothing happens to another buffer.
    pub fn set_agent_reads(&mut self, id: BufferId, reads: bool) {
        if let Some(buffer) = self
            .buffer_mut(id)
            .filter(|buffer| takes_session_flags(buffer))
        {
            buffer.set_agent_reads(reads);
        }
    }

    /// Notes whether the terminal buffer `id`, or a native session's, hosts an agent session
    /// (ADR 0132, ADR 0165): session mode's keys apply in it and its modeline says
    /// `Session`. Taking it back leaves the buffer outside every project again. Nothing
    /// happens to another buffer.
    pub fn set_hosts_session(&mut self, id: BufferId, hosts: bool) {
        let Some(buffer) = self
            .buffer_mut(id)
            .filter(|buffer| takes_session_flags(buffer))
        else {
            return;
        };
        buffer.set_session(hosts);
        if !hosts {
            buffer.set_project(None);
            self.project_requests.retain(|request| request.buffer != id);
        }
    }

    /// Renames the terminal buffer `id` to `base`, or `base<2>`, `base<3>` and on when another
    /// buffer has that name, as [`Editor::add_terminal_buffer`] names a new one: a hosted
    /// session's buffer after its session's rename (ADR 0155). Gives the name it took, or
    /// `None` for a buffer that is not a terminal's, which keeps its name.
    pub fn rename_terminal_buffer(&mut self, id: BufferId, base: &str) -> Option<String> {
        self.buffer(id).filter(|buffer| buffer.is_terminal())?;
        let name = self.free_terminal_name(base, Some(id));
        self.buffer_mut(id)?.set_name(name.clone());
        Some(name)
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
    fn a_terminal_is_renamed_apart_from_the_others() {
        let mut editor = Editor::default();
        let first = editor.add_terminal_buffer("*claude: api*");
        let second = editor.add_terminal_buffer("*claude: web*");
        let name = |editor: &Editor, id| editor.buffer(id).map(|b| b.name().to_owned()).unwrap();
        assert_eq!(
            editor.rename_terminal_buffer(second, "*claude: api*"),
            Some("*claude: api*<2>".into())
        );
        assert_eq!(name(&editor, second), "*claude: api*<2>");
        // Its own name is no clash.
        assert_eq!(
            editor.rename_terminal_buffer(first, "*claude: api*"),
            Some("*claude: api*".into())
        );
        let scratch = editor.window().buffer();
        let before = name(&editor, scratch);
        assert_eq!(editor.rename_terminal_buffer(scratch, "*claude: x*"), None);
        assert_eq!(name(&editor, scratch), before);
    }

    #[test]
    fn a_session_is_busy_by_its_turn_and_another_terminal_by_its_job() {
        let mut editor = Editor::default();
        let shell = editor.add_terminal_buffer("*terminal*");
        let session = editor.add_terminal_buffer("*claude*");
        editor.set_hosts_session(session, true);
        for id in [shell, session] {
            editor.set_terminal_job(id, true);
        }
        // Nothing runs yet, so nothing is busy.
        assert_eq!(editor.busy_terminals().count(), 0);
        editor.set_running(shell, true);
        editor.set_running(session, true);
        // A session's job is its own program's business; its turn decides.
        assert_eq!(editor.busy_terminals().collect::<Vec<_>>(), [shell]);
        editor.set_session_turn(session, true);
        editor.set_session_turn(shell, true);
        editor.set_terminal_job(shell, false);
        assert_eq!(editor.busy_terminals().collect::<Vec<_>>(), [session]);
        // An end forgets both.
        editor.set_running(session, false);
        editor.set_running(session, true);
        assert_eq!(editor.busy_terminals().count(), 0);
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
