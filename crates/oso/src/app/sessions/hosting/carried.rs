//! Hosted sessions carried over a restart (ADR 0120, step 11, ADR 0121, ADR 0133): what the
//! state says of the session a terminal runs, and hosting it again in the new instance with
//! its id, name, space, parent and baseline, running a `claude` that is not this instance's
//! child, whose start time its bridge's proof checks. The sessions worker is told of it as
//! of a start, so that its screens are judged.
//!
//! Must not depend on: sockets, or the terminal's screen.

use oso_core::buffer::BufferId;

use crate::app::App;
use crate::carry::terminals::{SessionState, TerminalState};
use crate::sessions::hosted::{Run, TuiProcess};
use crate::sessions::identity::{Origin, SessionId};
use crate::sessions::record::Remembered;
use crate::sessions::worker::Job;

impl App {
    /// What the state says of the session the terminal `buffer` runs, if it runs one.
    pub(in crate::app) fn carried_session_state(&self, buffer: BufferId) -> Option<SessionState> {
        let session = self
            .hosting
            .table
            .iter()
            .find(|s| s.buffer == Some(buffer))?;
        Some(SessionState {
            id: session.id.as_str().to_owned(),
            name: session.name.clone(),
            directory: session.directory.clone(),
            root: session.root.clone(),
            parent: session.parent.as_ref().map(|p| p.as_str().to_owned()),
            origin: Some(session.origin.name().to_owned()),
            baseline: session.baseline.clone(),
            pinned: session.pinned,
            changed: session.changed,
            launched_as: session.launched_as.clone(),
        })
    }

    /// Hosts the session a carried terminal runs as it was, and returns its buffer: the one
    /// it came back in stopped, if it did, else a new one. `None` if the terminal runs no
    /// session, or one whose id is not one: it comes back as a plain terminal.
    pub(in crate::app) fn host_carried_session(
        &mut self,
        terminal: &TerminalState,
    ) -> Option<BufferId> {
        let ids = terminal.session.as_ref().and_then(|session| {
            let parent = match &session.parent {
                Some(parent) => Some(SessionId::parse(parent)?),
                None => None,
            };
            Some((session, SessionId::parse(&session.id)?, parent))
        });
        let (session, id, parent) = ids?;
        let known = self
            .hosting
            .table
            .iter()
            .find(|s| s.id == id)
            .map(|s| (s.key, s.buffer));
        let key = match known {
            Some((key, _)) => key,
            None => {
                let back = self.hosting.table.recall(vec![Remembered {
                    id,
                    name: session.name.clone(),
                    directory: session.directory.clone(),
                    root: session.root.clone(),
                    origin: Origin::read(session.origin.as_deref(), parent.is_some()),
                    parent,
                    pinned: session.pinned,
                    archived: false,
                    native: false,
                    changed: session.changed,
                    baseline: session.baseline.clone(),
                    space: None,
                    reads: Vec::new(),
                }]);
                *back.first()?
            }
        };
        let stopped = known.and_then(|(_, buffer)| buffer);
        let buffer = match stopped {
            Some(buffer) if self.editor.buffer(buffer).is_some() => {
                self.terminals.map.restart(buffer);
                buffer
            }
            _ => {
                let buffer = self.editor.add_terminal_buffer(&terminal.name);
                self.terminals.map.add(buffer);
                buffer
            }
        };
        if let Some(hosted) = self.hosting.table.get_mut(key) {
            hosted.buffer = Some(buffer);
            hosted.run = Run::Running {
                child: terminal.pid,
            };
            hosted.carried = true;
            hosted.started = terminal.started;
            // Its `claude`, which a native resume waits to be gone for (ADR 0165, part 4d-2).
            hosted.tui_process = terminal.pid.map(|pid| TuiProcess {
                pid,
                started: terminal.started,
            });
            hosted.launched_as.clone_from(&session.launched_as);
        }
        // The worker judges only the sessions it was told of, as a start tells it: without
        // this, every screen of a carried session is dropped and its state never judged.
        if let Some(worker) = &self.sessions {
            worker.send(Job::Start(key));
        }
        // It stands where the session runs (ADR 0141).
        self.editor
            .set_buffer_directory(buffer, session.directory.clone());
        Some(buffer)
    }
}
