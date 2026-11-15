//! Hosted sessions carried over `oso dev`'s restart (ADR 0120, step 11, ADR 0121): what the
//! state says of the session a terminal runs, and hosting it again in the new instance with
//! its id, name, space, parent and baseline, running a `claude` that is not this instance's
//! child, whose start time its bridge's proof checks. Dev builds only.
//!
//! Must not depend on: sockets, or the terminal's screen.

use oso_core::buffer::BufferId;

use crate::app::App;
use crate::dev::handoff::terminals::{SessionState, TerminalState};
use crate::sessions::hosted::Run;
use crate::sessions::identity::{Origin, SessionId};
use crate::sessions::record::Remembered;

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
                    changed: session.changed,
                    baseline: session.baseline.clone(),
                    space: None,
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
        }
        Some(buffer)
    }
}
