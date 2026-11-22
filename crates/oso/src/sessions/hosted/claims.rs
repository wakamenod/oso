//! A hosted session's bridges (ADR 0121): which connection holds a session, on the agent
//! channel, and for a native session on the sessions' socket too, where its bridge asks the
//! person's permission (ADR 0165, step 2). Each claim is judged as `bridge::admit` has it.
//!
//! Must not depend on: `app`, `dispatch` or threads; methods of the table over its sessions.

use super::super::bridge::{self, Candidate};
use super::{Connection, Hosted, Run, SessionId, SessionKey, Table};

impl Hosted {
    /// The connection it holds in `slot`.
    fn slot(&self, slot: Slot) -> Option<Connection> {
        match slot {
            Slot::Channel => self.bridge,
            Slot::Permission => self.permission_bridge,
        }
    }

    fn slot_mut(&mut self, slot: Slot) -> &mut Option<Connection> {
        match slot {
            Slot::Channel => &mut self.bridge,
            Slot::Permission => &mut self.permission_bridge,
        }
    }
}

/// Which of a session's bridge connections: to the agent channel, or to the sessions' socket
/// for the person's permission (ADR 0165, step 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Channel,
    Permission,
}

/// What a claim on the sessions' socket came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionClaim {
    /// The session holds the connection now.
    Holds(SessionKey),
    /// The connection would be this session's bridge, but it holds another already.
    Held(SessionKey),
    No,
}

/// What a claim came to, by the session's index.
enum Claimed {
    Holds(usize),
    Held(usize),
    No,
}

impl Table {
    /// Whether the connection of `peer`, claiming `claim`, is a hosted session's bridge, as
    /// `bridge::admit` judges it. If so, that session holds it from now on.
    #[cfg(test)]
    pub fn admit(
        &mut self,
        connection: Connection,
        peer: &oso_platform::control::Peer,
        claim: Option<&SessionId>,
        parent_started: Option<std::time::SystemTime>,
    ) -> Option<&Hosted> {
        let index = self.admit_in(Slot::Channel, connection, peer, (claim, parent_started))?;
        Some(&self.sessions[index])
    }

    /// The session of `claim` that `peer`'s connection is the bridge of, by `bridge::admit`
    /// over the sessions that may hold one in `slot`; it holds `connection` there from now on.
    fn admit_in(
        &mut self,
        slot: Slot,
        connection: Connection,
        peer: &oso_platform::control::Peer,
        (claim, parent_started): (Option<&SessionId>, Option<std::time::SystemTime>),
    ) -> Option<usize> {
        let index = bridge::admit(peer, claim, parent_started, self.candidates(slot, true))?;
        *self.sessions[index].slot_mut(slot) = Some(connection);
        Some(index)
    }

    /// The sessions as candidates for `slot`: a running session's `claude`, for the
    /// permission slot only a native one's (ADR 0165, step 2); one that holds a connection
    /// there is `bridged` unless not `held_counts`.
    fn candidates(&self, slot: Slot, held_counts: bool) -> impl Iterator<Item = Candidate<'_>> {
        self.sessions.iter().map(move |session| Candidate {
            id: &session.id,
            child: match session.run {
                Run::Running { child } if slot == Slot::Channel || session.native => child,
                _ => None,
            },
            bridged: held_counts && session.slot(slot).is_some(),
            // A carried session is a terminal's, never native.
            carried: session.carried,
            started: session.started,
        })
    }

    /// Answers the claim of `connection`, from `peer`, to be `claim`'s bridge: [`Table::admit`],
    /// then `answer` with whether it was admitted, which says whether the answer reached the
    /// one who asked. An admission whose answer did not reach is undone: the asker took the
    /// connection as no bridge. The session's name if it holds the connection now.
    pub fn answer_claim(
        &mut self,
        connection: Connection,
        peer: &oso_platform::control::Peer,
        claim: Option<&SessionId>,
        parent_started: Option<std::time::SystemTime>,
        answer: impl FnOnce(bool) -> bool,
    ) -> Option<String> {
        let claimed = self.answer_in(
            Slot::Channel,
            connection,
            peer,
            (claim, parent_started),
            answer,
        );
        match claimed {
            Claimed::Holds(index) => Some(self.sessions[index].name.clone()),
            Claimed::Held(_) | Claimed::No => None,
        }
    }

    /// As [`Table::answer_claim`], for the connection `connection` to the sessions' socket of
    /// a running native session's bridge, which asks the person's permission for the session's
    /// calls (ADR 0165, step 2). Says whether the session holds it now, or held another, which
    /// is what a second claimant of a session that passes the claim, such as a command the
    /// session runs, is told.
    pub fn answer_permission_claim(
        &mut self,
        connection: Connection,
        peer: &oso_platform::control::Peer,
        claim: Option<&SessionId>,
        parent_started: Option<std::time::SystemTime>,
        answer: impl FnOnce(bool) -> bool,
    ) -> PermissionClaim {
        let claimed = self.answer_in(
            Slot::Permission,
            connection,
            peer,
            (claim, parent_started),
            answer,
        );
        match claimed {
            Claimed::Holds(index) => PermissionClaim::Holds(self.sessions[index].key),
            Claimed::Held(index) => PermissionClaim::Held(self.sessions[index].key),
            Claimed::No => PermissionClaim::No,
        }
    }

    fn answer_in(
        &mut self,
        slot: Slot,
        connection: Connection,
        peer: &oso_platform::control::Peer,
        claim: (Option<&SessionId>, Option<std::time::SystemTime>),
        answer: impl FnOnce(bool) -> bool,
    ) -> Claimed {
        let index = self.admit_in(slot, connection, peer, claim);
        let reached = answer(index.is_some());
        match index {
            Some(index) if reached => Claimed::Holds(index),
            Some(index) => {
                *self.sessions[index].slot_mut(slot) = None;
                Claimed::No
            }
            // The one it would have been, had the slot been free.
            None => bridge::admit(peer, claim.0, claim.1, self.candidates(slot, false))
                .map_or(Claimed::No, Claimed::Held),
        }
    }

    /// The session whose permission bridge `connection` is, if one is.
    pub fn by_permission_bridge(&self, connection: Connection) -> Option<&Hosted> {
        self.sessions
            .iter()
            .find(|session| session.permission_bridge == Some(connection))
    }

    /// The sessions' socket stopped: no session holds a connection to it.
    pub fn permission_bridges_gone(&mut self) {
        for session in &mut self.sessions {
            session.permission_bridge = None;
        }
    }

    /// The sessions' socket's connection `connection` went: no session holds it.
    pub fn permission_bridge_gone(&mut self, connection: Connection) {
        for session in &mut self.sessions {
            if session.permission_bridge == Some(connection) {
                session.permission_bridge = None;
            }
        }
    }

    /// The bridge connection `connection` went: its session holds none.
    pub fn bridge_gone(&mut self, connection: Connection) {
        for session in &mut self.sessions {
            if session.bridge == Some(connection) {
                session.bridge = None;
            }
        }
    }

    /// The channel stopped: no session holds a connection.
    pub fn bridges_gone(&mut self) {
        for session in &mut self.sessions {
            session.bridge = None;
        }
    }
}
