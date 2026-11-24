//! Why Remote Control is not turned on for a session (ADR 0181): looked at as the person asks,
//! again at their `y`, and, for what may differ after a restart, as it would connect again.
//!
//! Must not depend on: `app`, threads or the UI; the caller says where the session is.

use super::super::mode;

/// Where a session is, as the caller knows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct At {
    pub whose: Whose,
    /// It runs a process, or is being started.
    pub runs: bool,
    pub starting: bool,
    /// Where its process is, for a respawn.
    pub respawn: Option<mode::Refusal>,
    /// It asks the person, is being interrupted, or its mode, the mode it runs in or the
    /// repositories it is given are being changed.
    pub asks: bool,
    pub changing: bool,
    /// Its transcript or its composer holds secret text.
    pub secret: bool,
    /// Its project is not trusted now.
    pub untrusted: bool,
}

/// Whose a session is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Whose {
    /// An agent started it, or it is a fork of one that an agent started.
    pub agents: bool,
    /// It is an issue space's orchestrator, or was one, or runs in a space's own directory.
    pub orchestrator: bool,
}

/// Why Remote Control is not turned on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Agents,
    Orchestrator,
    NotRunning,
    Starting,
    Asks,
    Changing,
    Turn,
    Unspoken,
    Held,
    Secret,
    Untrusted,
}

impl Whose {
    /// Why it is never connected.
    pub fn refused(self) -> Option<Refused> {
        if self.agents {
            return Some(Refused::Agents);
        }
        self.orchestrator.then_some(Refused::Orchestrator)
    }
}

impl At {
    /// Why it is not turned on now.
    pub fn refused(self) -> Option<Refused> {
        use mode::Refusal::{Busy, Unspoken};
        if let Some(refused) = self.whose.refused() {
            return Some(refused);
        }
        let first = [
            (self.starting, Refused::Starting),
            (!self.runs, Refused::NotRunning),
            (self.asks, Refused::Asks),
            (self.changing, Refused::Changing),
        ];
        if let Some((_, refused)) = first.into_iter().find(|(is, _)| *is) {
            return Some(refused);
        }
        match self.respawn {
            Some(Busy) => return Some(Refused::Turn),
            Some(Unspoken) => return Some(Refused::Unspoken),
            Some(_) => return Some(Refused::Held),
            None => {}
        }
        if self.secret {
            return Some(Refused::Secret);
        }
        self.untrusted.then_some(Refused::Untrusted)
    }
}

impl Refused {
    /// What the person is told of session `name`.
    pub fn message(self, name: &str) -> String {
        let why = match self {
            Self::Agents => "was started by an agent: it is never connected to claude.ai",
            Self::Orchestrator => {
                "is an orchestrator, or runs in a space's directory: it is not connected to \
                 claude.ai yet"
            }
            Self::NotRunning => "runs no claude: resume it first",
            Self::Starting => "is starting: ask again once it runs",
            Self::Asks => "asks you something: answer it first",
            Self::Changing => "is being changed: ask again once that is over",
            Self::Turn => "has a turn under way: ask again once it is idle",
            Self::Unspoken => {
                "was never given a message, so it cannot be resumed to connect it: send one first"
            }
            Self::Held => {
                "cannot be resumed now, as connecting it needs: a stop is asked, oso waits to \
                 quit or restart, or its conversation moved"
            }
            Self::Secret => "holds secret text in its composer or transcript: take it out first",
            Self::Untrusted => "is in a project that is not trusted now",
        };
        format!("Session {name} {why}")
    }
}

#[cfg(test)]
mod tests {
    use super::{At, Refused, Whose};
    use crate::sessions::native::mode::Refusal;

    const FREE: At = At {
        whose: Whose {
            agents: false,
            orchestrator: false,
        },
        runs: true,
        starting: false,
        respawn: None,
        asks: false,
        changing: false,
        secret: false,
        untrusted: false,
    };

    #[test]
    fn each_thing_that_holds_it_refuses_and_says_so() {
        assert_eq!(FREE.refused(), None);
        let whose = |agents, orchestrator| Whose {
            agents,
            orchestrator,
        };
        let with = |change: fn(&mut At)| {
            let mut at = FREE;
            change(&mut at);
            at
        };
        let agents = At {
            whose: whose(true, false),
            ..FREE
        };
        let orchestrator = At {
            whose: whose(false, true),
            ..FREE
        };
        let cases = [
            (agents, Refused::Agents, "by an agent"),
            (orchestrator, Refused::Orchestrator, "orchestrator"),
            (with(|at| at.runs = false), Refused::NotRunning, "resume it"),
            (with(|at| at.starting = true), Refused::Starting, "starting"),
            (with(|at| at.asks = true), Refused::Asks, "answer it"),
            (with(|at| at.changing = true), Refused::Changing, "changed"),
            (
                with(|at| at.respawn = Some(Refusal::Busy)),
                Refused::Turn,
                "a turn under way",
            ),
            (
                with(|at| at.respawn = Some(Refusal::Unspoken)),
                Refused::Unspoken,
                "never given a message",
            ),
            (
                with(|at| at.respawn = Some(Refusal::Held)),
                Refused::Held,
                "cannot be resumed",
            ),
            (with(|at| at.secret = true), Refused::Secret, "secret"),
            (
                with(|at| at.untrusted = true),
                Refused::Untrusted,
                "not trusted",
            ),
        ];
        for (at, refused, says) in cases {
            assert_eq!(at.refused(), Some(refused));
            let said = refused.message("wa");
            assert!(said.starts_with("Session wa "), "{said}");
            assert!(said.contains(says), "{said}");
        }
    }

    #[test]
    fn whose_it_is_is_said_before_where_it_is() {
        let at = At {
            whose: Whose {
                agents: true,
                orchestrator: true,
            },
            runs: false,
            ..FREE
        };
        assert_eq!(at.refused(), Some(Refused::Agents));
    }
}
