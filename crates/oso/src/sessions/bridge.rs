//! Which hosted session a bridge belongs to (ADR 0121, "The bridge and the channel", Q19): the
//! one decision that lets a connection in without asking the person.
//!
//! Must not depend on: sockets, threads or `app`; the channel's server reads the peer and the
//! claim, the app holds the sessions, and this only judges them.
//!
//! `oso mcp` names its session with `OSO_SESSION` in its first request. The claim is believed
//! only if all of these hold, else the connection has no session and asks as ADR 0085 has it:
//!
//! - the process that connected runs this program;
//! - its parent is the session's `claude`, whose process id oso holds as its child, so the id
//!   cannot be reused while oso has not reaped it. Parent only: Claude Code starts a stdio MCP
//!   server itself;
//! - the id claimed is that session's;
//! - the session's terminal has not ended;
//! - no other connection holds that session already.

use oso_platform::control::{Peer, PeerProcess};

use super::identity::SessionId;

/// A hosted session as the decision sees it.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub id: &'a SessionId,
    /// Its `claude`'s process id while it runs and is not reaped.
    pub child: Option<u32>,
    /// A connection holds this session already.
    pub bridged: bool,
}

/// The session among `sessions` that the connection of `peer`, claiming `claim`, belongs to,
/// by its index; `None` if it must ask the person.
pub fn admit<'a>(
    peer: &Peer,
    claim: Option<&SessionId>,
    sessions: impl IntoIterator<Item = Candidate<'a>>,
) -> Option<usize> {
    let claim = claim?;
    if !peer.runs_this_program {
        return None;
    }
    let parent = peer.parent.as_ref().map(PeerProcess::pid)?;
    sessions.into_iter().position(|session| {
        session.id == claim && session.child == Some(parent) && !session.bridged
    })
}

#[cfg(test)]
mod tests {
    use oso_platform::control::{Peer, PeerProcess};

    use super::{Candidate, admit};
    use crate::sessions::identity::SessionId;

    /// What a case is called, who connects, what it claims, the sessions, and the one let in.
    type Case<'a> = (
        &'a str,
        Peer,
        Option<&'a SessionId>,
        &'a [Candidate<'a>],
        Option<usize>,
    );

    fn peer(runs_this_program: bool, parent: Option<u32>) -> Peer {
        Peer {
            process: Some(PeerProcess::new(900, Some("oso"))),
            parent: parent.map(|pid| PeerProcess::new(pid, Some("claude"))),
            runs_this_program,
        }
    }

    #[test]
    fn only_a_bridge_started_by_its_own_session_is_let_in_unasked() {
        let (a, b) = (
            SessionId::from_random([1; 16]),
            SessionId::from_random([2; 16]),
        );
        let sessions = [
            Candidate {
                id: &a,
                child: Some(100),
                bridged: false,
            },
            Candidate {
                id: &b,
                child: Some(200),
                bridged: false,
            },
        ];
        let ended = [Candidate {
            id: &a,
            child: None,
            bridged: false,
        }];
        let bridged = [Candidate {
            id: &a,
            child: Some(100),
            bridged: true,
        }];
        let cases: [Case<'_>; 9] = [
            (
                "its own",
                peer(true, Some(100)),
                Some(&a),
                &sessions,
                Some(0),
            ),
            (
                "the other",
                peer(true, Some(200)),
                Some(&b),
                &sessions,
                Some(1),
            ),
            ("no claim", peer(true, Some(100)), None, &sessions, None),
            (
                "another's id",
                peer(true, Some(100)),
                Some(&b),
                &sessions,
                None,
            ),
            ("not oso", peer(false, Some(100)), Some(&a), &sessions, None),
            ("no parent", peer(true, None), Some(&a), &sessions, None),
            (
                "not the child",
                peer(true, Some(101)),
                Some(&a),
                &sessions,
                None,
            ),
            ("ended", peer(true, Some(100)), Some(&a), &ended, None),
            (
                "held already",
                peer(true, Some(100)),
                Some(&a),
                &bridged,
                None,
            ),
        ];
        for (case, peer, claim, sessions, expected) in cases {
            assert_eq!(
                admit(&peer, claim, sessions.iter().copied()),
                expected,
                "{case}"
            );
        }
    }
}
