//! A hosted session's terminal (ADR 0121, step 1): its buffer `*claude: NAME*`, made at once
//! and given its terminal when the start thread started it, its end, the stopped buffers a
//! start of oso brings back, and `C-x k`, which archives the session.
//!
//! Must not depend on: key presses or how a frame is drawn, beyond a window's size.

use std::path::Path;

use oso_core::buffer::BufferId;
use oso_term::End;

use super::now_ms;
use crate::app::{App, UserEvent};
use crate::sessions::hosted::Run;
use crate::sessions::start::{Ready, TerminalOrder};
use crate::sessions::state::Exit;
use crate::sessions::worker::{Job, SessionKey};
use crate::terminal::hosted;

/// What a stopped session's modeline says.
pub(super) const STOPPED: &str = "stopped: session-resume to continue";

/// What the modeline of a buffer says whose session oso no longer keeps, past the most it
/// keeps.
const FORGOTTEN: &str = "forgotten: oso keeps the 256 sessions that changed last";

/// A session buffer's name.
fn buffer_name(name: &str) -> String {
    format!("*claude: {name}*")
}

impl App {
    /// The terminal session `key`, named `name`, starts in: `reuse`'s buffer, started over, or
    /// a new one. Either is shown, unless not `shown` (an agent's start, ADR 0144), and says it is
    /// starting until the start thread reports.
    pub(super) fn session_terminal(
        &mut self,
        key: SessionKey,
        name: &str,
        reuse: Option<BufferId>,
        shown: bool,
    ) -> TerminalOrder {
        let buffer = match reuse.filter(|id| self.editor.buffer(*id).is_some()) {
            Some(id) => {
                self.terminals.map.restart(id);
                self.terminal_restarted(id);
                id
            }
            None => {
                let id = self.editor.add_terminal_buffer(&buffer_name(name));
                self.terminals.map.add(id);
                id
            }
        };
        self.mark_session_buffer(key, buffer);
        // Running from now on, as a terminal is while its shell is found: an exit asks first.
        let size = self.prepare_terminal(buffer, shown);
        self.editor.set_mode_note(buffer, String::new());
        self.hosting.starting.insert(key, buffer);
        let ready = self.waker.clone();
        TerminalOrder {
            buffer,
            settings: self.terminal_settings(),
            env: self.configuration.session_env().to_vec(),
            size,
            palette: self.terminal_palette(),
            ready: Box::new(move || {
                ready.send(UserEvent::Terminal(buffer));
            }),
        }
    }

    /// The start thread started session `key`'s terminal: its buffer takes it. Returns
    /// whether it did; a buffer killed meanwhile drops it, which hangs it up.
    pub(super) fn session_terminal_started(&mut self, key: SessionKey, ready: Ready) -> bool {
        self.hosting.starting.remove(&key);
        let Ready {
            buffer,
            terminal,
            shell,
            passed_over,
            directory,
            ..
        } = ready;
        let child = terminal.pid();
        if !self.install_terminal(buffer, terminal, &passed_over) {
            return false;
        }
        if let Some(hosted) = self.terminals.map.get_mut(buffer) {
            hosted.note_stale = true;
        }
        let Some(session) = self.hosting.table.get_mut(key) else {
            return false;
        };
        session.buffer = Some(buffer);
        session.run = Run::Running { child };
        session.archived = false;
        let name = session.name.clone();
        // The path only: the environment's values are never shown (ADR 0120).
        self.editor.note(format!(
            "Started session {name} with {} in {}",
            shell.display(),
            directory.display()
        ));
        if let Some(worker) = &self.sessions {
            worker.send(Job::Start(key));
        }
        true
    }

    /// Session `key` did not start: a new buffer goes, and a resumed one is stopped again.
    pub(super) fn session_terminal_not_started(&mut self, key: SessionKey) {
        let Some(buffer) = self.hosting.starting.remove(&key) else {
            return;
        };
        let kept = self
            .hosting
            .table
            .get(key)
            .is_some_and(|session| session.buffer == Some(buffer));
        self.editor.set_running(buffer, false);
        if kept {
            self.show_stopped(buffer, STOPPED);
        } else {
            self.editor.kill_buffer(buffer);
        }
    }

    /// Before a terminal's handle is dropped: the program of `buffer` ended as `end`. If it is
    /// a session's, the session is stopped or failed, and its modeline says how to go on.
    pub(in crate::app) fn session_terminal_ended(&mut self, buffer: BufferId, end: &End) {
        let Some(key) = self.hosting.table.by_buffer(buffer).map(|s| s.key) else {
            return;
        };
        let status = match end {
            End::Exited(status) => status.code(),
            End::Closed => None,
            // A terminal that failed is a session that failed.
            End::Failed(_) => Some(-1),
        };
        // A hang-up the session list asked for ends it as stopped (ADR 0121, step 3).
        let status = if self.watch.take_hung_up(key) {
            Some(0)
        } else {
            status
        };
        let Some(exit) = self.hosting.table.ended(key, status, now_ms()) else {
            return;
        };
        let note = match exit {
            Exit::Stopped => STOPPED.to_owned(),
            Exit::Failed => format!(
                "failed ({}): session-resume to continue",
                hosted::ended(end)
            ),
        };
        self.show_stopped(buffer, &note);
        if let Some(worker) = &self.sessions {
            worker.send(Job::Exited(key, exit));
        }
        self.remember_sessions();
        // An orchestrator stopped for its successor: that starts now (ADR 0136, step 5).
        self.replaced_orchestrator_ended(key, matches!(end, End::Exited(_)));
    }

    /// Brings session `key` back as a terminal buffer with no process, which says
    /// [`STOPPED`]. Nothing starts.
    pub(super) fn bring_back_stopped(&mut self, key: SessionKey) {
        let Some(name) = self.hosting.table.get(key).map(|s| s.name.clone()) else {
            return;
        };
        let buffer = self.editor.add_terminal_buffer(&buffer_name(&name));
        self.terminals.map.add(buffer);
        self.editor.set_running(buffer, false);
        self.show_stopped(buffer, STOPPED);
        if let Some(session) = self.hosting.table.get_mut(key) {
            session.buffer = Some(buffer);
            session.run = Run::Stopped;
        }
        self.mark_session_buffer(key, buffer);
    }

    /// `buffer` is session `key`'s terminal: session mode's keys apply in it, its modeline says
    /// `Session`, and it is in the project of the directory the session runs in (ADR 0132).
    fn mark_session_buffer(&mut self, key: SessionKey, buffer: BufferId) {
        // A new session is only reserved yet, with its directory.
        let Some(directory) = self.hosting.table.directory(key).map(Path::to_path_buf) else {
            return;
        };
        self.editor.set_hosts_session(buffer, true);
        self.editor.request_directory_project(buffer, &directory);
    }

    /// `buffer`'s terminal has no program, and its modeline says `note`.
    /// Keeps the table within its cap: a buffer whose session went past it says so.
    pub(super) fn prune_sessions(&mut self) {
        for (id, buffer) in self.hosting.table.prune() {
            // The images of a session dropped go with it (ADR 0142).
            self.drop_session_images(id);
            let Some(buffer) = buffer else {
                continue;
            };
            self.show_stopped(buffer, FORGOTTEN);
            // No session to act on any more: a plain terminal's keys (ADR 0132).
            self.editor.set_hosts_session(buffer, false);
        }
    }

    fn show_stopped(&mut self, buffer: BufferId, note: &str) {
        if let Some(hosted) = self.terminals.map.get_mut(buffer) {
            hosted.ended = Some(note.to_owned());
            hosted.note_stale = true;
        }
    }

    /// The terminals of `buffers` went with their buffers (`C-x k`): their sessions are
    /// archived, as oso hangs their `claude` up, and do not come back at the next start
    /// (ADR 0121, Q3).
    pub(in crate::app) fn archive_killed_sessions(&mut self, buffers: &[BufferId]) {
        let gone: Vec<(SessionKey, bool, crate::sessions::identity::SessionId)> = self
            .hosting
            .table
            .iter()
            .filter(|session| {
                session
                    .buffer
                    .is_some_and(|buffer| buffers.contains(&buffer))
            })
            .map(|session| (session.key, session.runs(), session.id.clone()))
            .collect();
        if gone.is_empty() {
            return;
        }
        for (key, ran, id) in gone {
            // An archived session's images go (ADR 0142).
            self.drop_session_images(id);
            if let Some(session) = self.hosting.table.get_mut(key) {
                session.buffer = None;
                session.archived = true;
                session.bridge = None;
                session.run = Run::Stopped;
                session.changed = now_ms();
            }
            if ran && let Some(worker) = &self.sessions {
                worker.send(Job::Exited(key, Exit::Stopped));
            }
        }
        self.prune_sessions();
        self.remember_sessions();
    }
}
