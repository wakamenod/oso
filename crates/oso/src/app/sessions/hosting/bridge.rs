//! The bridge connections that hold a hosted session (ADR 0121): a claim is answered from
//! the running `claude`s' process ids as their terminals have them.
//!
//! Must not depend on: key presses or how a frame is drawn.

use oso_platform::control::Peer;

use crate::app::App;
use crate::channel::Connection;
use crate::sessions::hosted::Run;
use crate::sessions::identity::SessionId;
use crate::sessions::worker::SessionKey;

impl App {
    /// Whether the connection of `peer` that claims the session `claim` is that hosted
    /// session's bridge (ADR 0121), answered with `answer`, which says whether the answer
    /// reached the server: then it holds the session, and its name is given.
    pub(in crate::app) fn session_bridge(
        &mut self,
        connection: Connection,
        peer: &Peer,
        claim: &str,
        answer: impl FnOnce(bool) -> bool,
    ) -> Option<String> {
        let Some(id) = SessionId::parse(claim) else {
            answer(false);
            return None;
        };
        // Each running `claude`'s id as its terminal has it now: set once the child started,
        // cleared before it is reaped.
        let children: Vec<(SessionKey, Option<u32>)> = self
            .hosting
            .table
            .iter()
            .filter(|session| matches!(session.run, Run::Running { .. }))
            .map(|session| {
                let child = session
                    .buffer
                    .and_then(|buffer| self.terminals.map.get(buffer))
                    .and_then(|hosted| hosted.terminal.as_ref())
                    .and_then(oso_term::Terminal::pid);
                (session.key, child)
            })
            .collect();
        for (key, child) in children {
            if let Some(session) = self.hosting.table.get_mut(key) {
                session.run = Run::Running { child };
            }
        }
        self.hosting
            .table
            .answer_claim(connection, peer, Some(&id), answer)
    }

    /// The bridge connection `connection` went, or every one did, when it is `None`.
    pub(in crate::app) fn session_bridge_gone(&mut self, connection: Option<Connection>) {
        match connection {
            Some(connection) => self.hosting.table.bridge_gone(connection),
            None => self.hosting.table.bridges_gone(),
        }
    }
}
