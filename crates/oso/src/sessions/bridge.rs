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
//! - no other connection holds that session already;
//! - for a session carried over `oso dev`'s restart, whose `claude` is no longer oso's child,
//!   the parent also started when that `claude` did, so a process that took its id after it
//!   ended is not believed (ADR 0120, step 11).

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
    /// Whether its `claude` is not oso's child: a session carried over `oso dev`'s restart.
    pub carried: bool,
    /// When that `claude` started, if the system said.
    pub started: Option<std::time::SystemTime>,
}

/// The session among `sessions` that the connection of `peer`, claiming `claim`, belongs to,
/// by its index; `None` if it must ask the person.
pub fn admit<'a>(
    peer: &Peer,
    claim: Option<&SessionId>,
    parent_started: Option<std::time::SystemTime>,
    sessions: impl IntoIterator<Item = Candidate<'a>>,
) -> Option<usize> {
    let claim = claim?;
    if !peer.runs_this_program {
        return None;
    }
    let parent = peer.parent.as_ref().map(PeerProcess::pid)?;
    sessions.into_iter().position(|session| {
        session.id == claim
            && session.child == Some(parent)
            && !session.bridged
            && (!session.carried
                || session
                    .started
                    .is_some_and(|started| parent_started == Some(started)))
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

    /// A session oso started, its `claude` its own child.
    fn own(id: &SessionId, child: Option<u32>, bridged: bool) -> Candidate<'_> {
        Candidate {
            id,
            child,
            bridged,
            carried: false,
            started: None,
        }
    }

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
        let sessions = [own(&a, Some(100), false), own(&b, Some(200), false)];
        let ended = [own(&a, None, false)];
        let bridged = [own(&a, Some(100), true)];
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
                admit(&peer, claim, None, sessions.iter().copied()),
                expected,
                "{case}"
            );
        }
    }

    #[test]
    fn a_carried_sessions_bridge_must_have_the_parent_that_started_then() {
        use std::time::{Duration, SystemTime};
        let a = SessionId::from_random([1; 16]);
        let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let carried = [Candidate {
            id: &a,
            child: Some(100),
            bridged: false,
            carried: true,
            started: Some(then),
        }];
        let by = peer(true, Some(100));
        let admitted = |started| admit(&by, Some(&a), started, carried.iter().copied());
        assert_eq!(admitted(Some(then)), Some(0));
        assert_eq!(
            admitted(Some(then + Duration::from_secs(1))),
            None,
            "a later process with the same id"
        );
        assert_eq!(admitted(None), None, "a start the system did not say");
        let unknown = [Candidate {
            started: None,
            ..carried[0]
        }];
        assert_eq!(
            admit(&by, Some(&a), Some(then), unknown.iter().copied()),
            None,
            "a carried session whose start is unknown lets no bridge in unasked"
        );
    }
}
