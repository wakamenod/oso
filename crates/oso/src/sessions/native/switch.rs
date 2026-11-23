//! A session's switch between its modes, native and the TUI (ADR 0165, step 4, part 4d):
//! `session-switch-mode`'s rules, and where each switch is, from the end of the process of one
//! mode to the start of the other's. These are the rules; the caller ends and starts the
//! processes, and says when each ended or started.
//!
//! Must not depend on: `app`, threads, the clock or the process.
//!
//! - **Never two writers.** A switch ends the process of the mode the session is in, and the
//!   other mode starts only once that process ended and was reaped. That start is a resume,
//!   whose start thread reads Claude Code's registry and refuses a live process of the
//!   conversation elsewhere (`live`), as every resume does.
//! - **When.** A native session switches while it is idle, done or stopped: not while its
//!   process starts, a turn is under way, it is being interrupted, its permission mode is
//!   being changed, it asks the person (a permission, question or plan card, a plan's edit
//!   among them), or messages are queued, which would be lost to the TUI. A session never
//!   given a message has no conversation to resume, one whose conversation moved to an id oso
//!   does not follow would resume the one before, and none switches while a resume is held: a
//!   stop asked, oso waiting to quit or restarting, an orchestrator being replaced.
//! - **One at a time.** A session switches once until its switch is over: the other mode
//!   started, or did not.

use std::collections::HashMap;

/// Which way a session switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// From native to the TUI (part 4d-1).
    ToTui,
}

/// Where a native session is, as the caller knows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativeAt {
    /// Its process runs: it is idle or done, and ends first.
    pub runs: bool,
    /// Its process is being started.
    pub starting: bool,
    /// A turn is under way, as its stream says.
    pub turn: bool,
    /// It asks the person: a permission, question or plan card, or a plan being edited.
    pub card: bool,
    /// It is being interrupted (step 2c).
    pub interrupting: bool,
    /// Its permission mode is being changed (part 3b).
    pub mode_changing: bool,
    /// How many messages are queued (part 2a).
    pub queued: usize,
    /// It was given a message, so Claude Code keeps a conversation to resume.
    pub spoke: bool,
    /// Its conversation moved to a session id oso does not follow (part 2d).
    pub moved: bool,
    /// A resume of it is held now: a stop asked, oso waiting to quit or restarting, or an
    /// orchestrator being replaced.
    pub held: bool,
}

/// Why a session does not switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Already,
    Starting,
    Turn,
    Card,
    Interrupting,
    ModeChanging,
    Queued(usize),
    Unspoken,
    Moved,
    Held,
}

impl Refusal {
    /// What the person is told of session `name`.
    pub fn message(self, name: &str) -> String {
        match self {
            Self::Already => format!("Session {name} is switching already"),
            Self::Starting => format!("Session {name} is starting: it switches once it runs"),
            Self::Turn => format!("Session {name} has a turn under way: it switches once idle"),
            Self::Card => format!("Session {name} asks you something: answer it first"),
            Self::Interrupting => {
                format!("Session {name} is being interrupted: it switches once idle")
            }
            Self::ModeChanging => format!(
                "Session {name}'s permission mode is being changed: it switches once that is over"
            ),
            Self::Queued(1) => {
                format!("Session {name} has a queued message: send or drop it first")
            }
            Self::Queued(n) => {
                format!("Session {name} has {n} queued messages: send or drop them first")
            }
            Self::Unspoken => format!(
                "Session {name} was never given a message, so Claude Code has no conversation \
                 to take to the TUI"
            ),
            Self::Moved => format!(
                "Session {name}'s conversation moved to a session id oso does not follow: \
                 the TUI would resume the conversation before"
            ),
            Self::Held => format!(
                "Session {name} does not switch now: a stop is asked, or oso waits to quit or \
                 restart"
            ),
        }
    }
}

/// What the caller does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    /// End the process, and say when it ended and was reaped ([`Switches::ended`]).
    End,
    /// No process runs: start the other mode now.
    Start,
}

/// Where a switch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The process of the mode the session was in is being ended.
    Ending,
    /// The other mode is being started.
    Starting,
}

/// The sessions switching, by the caller's key.
#[derive(Debug, Default)]
pub struct Switches {
    map: HashMap<u64, (Way, Phase)>,
}

impl Switches {
    /// `session-switch-mode` for the native session `key`, which is `at`: to the TUI, ending
    /// its process first if it runs.
    ///
    /// # Errors
    ///
    /// Why it does not switch now.
    pub fn hand_to_tui(&mut self, key: u64, at: &NativeAt) -> Result<Then, Refusal> {
        let refusal = if self.map.contains_key(&key) {
            Some(Refusal::Already)
        } else if at.moved {
            Some(Refusal::Moved)
        } else if !at.spoke {
            Some(Refusal::Unspoken)
        } else if at.held {
            Some(Refusal::Held)
        } else if at.starting {
            Some(Refusal::Starting)
        } else if at.interrupting {
            Some(Refusal::Interrupting)
        } else if at.mode_changing {
            Some(Refusal::ModeChanging)
        } else if at.card {
            Some(Refusal::Card)
        } else if at.turn && at.runs {
            Some(Refusal::Turn)
        } else if at.queued > 0 {
            Some(Refusal::Queued(at.queued))
        } else {
            None
        };
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        Ok(self.begin(key, Way::ToTui, at.runs))
    }

    /// The switch begins: its process ends first if it `runs`.
    fn begin(&mut self, key: u64, way: Way, runs: bool) -> Then {
        let (phase, then) = match runs {
            true => (Phase::Ending, Then::End),
            false => (Phase::Starting, Then::Start),
        };
        self.map.insert(key, (way, phase));
        then
    }

    /// Which way session `key` switches while its process is being ended for it.
    pub fn ending(&self, key: u64) -> Option<Way> {
        match self.map.get(&key) {
            Some(&(way, Phase::Ending)) => Some(way),
            _ => None,
        }
    }

    /// Session `key`'s process ended and was reaped: if a switch ended it, the other mode
    /// starts now, the way this gives.
    pub fn ended(&mut self, key: u64) -> Option<Way> {
        let (way, phase) = self.map.get_mut(&key)?;
        if *phase != Phase::Ending {
            return None;
        }
        *phase = Phase::Starting;
        Some(*way)
    }

    /// Which way session `key` switches while the other mode is being started.
    pub fn starting(&self, key: u64) -> Option<Way> {
        match self.map.get(&key) {
            Some(&(way, Phase::Starting)) => Some(way),
            _ => None,
        }
    }

    /// Session `key`'s switch is over: the other mode started, did not, or the session went.
    /// Which way it was.
    pub fn over(&mut self, key: u64) -> Option<Way> {
        self.map.remove(&key).map(|(way, _)| way)
    }

    /// Whether session `key` is switching.
    pub fn under_way(&self, key: u64) -> bool {
        self.map.contains_key(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeAt, Refusal, Switches, Then, Way};

    fn idle() -> NativeAt {
        NativeAt {
            runs: true,
            spoke: true,
            ..NativeAt::default()
        }
    }

    #[test]
    fn an_idle_native_session_ends_its_process_then_starts_the_tui() {
        let mut switches = Switches::default();
        assert_eq!(switches.hand_to_tui(1, &idle()), Ok(Then::End));
        assert_eq!(switches.ending(1), Some(Way::ToTui));
        assert_eq!(switches.starting(1), None);
        // One at a time.
        assert_eq!(switches.hand_to_tui(1, &idle()), Err(Refusal::Already));
        assert_eq!(switches.ended(1), Some(Way::ToTui));
        assert_eq!(switches.ending(1), None);
        assert_eq!(switches.starting(1), Some(Way::ToTui));
        // An end it did not cause, later, starts nothing.
        assert_eq!(switches.ended(1), None);
        assert_eq!(switches.over(1), Some(Way::ToTui));
        assert!(!switches.under_way(1));
        assert_eq!(switches.over(1), None);
    }

    #[test]
    fn a_stopped_native_session_starts_the_tui_at_once() {
        let mut switches = Switches::default();
        let stopped = NativeAt {
            runs: false,
            ..idle()
        };
        assert_eq!(switches.hand_to_tui(2, &stopped), Ok(Then::Start));
        assert_eq!(switches.ending(2), None);
        assert_eq!(switches.starting(2), Some(Way::ToTui));
        // Its last turn as its stream said it holds nothing back once its process ended.
        let mut switches = Switches::default();
        let ended_mid_turn = NativeAt {
            turn: true,
            ..stopped
        };
        assert_eq!(switches.hand_to_tui(3, &ended_mid_turn), Ok(Then::Start));
        // An end no switch asked for starts nothing.
        assert_eq!(switches.ended(4), None);
    }

    /// An idle session, changed by `change`.
    fn idle_but(change: impl Fn(&mut NativeAt)) -> NativeAt {
        let mut at = idle();
        change(&mut at);
        at
    }

    #[test]
    fn a_native_session_that_is_not_idle_does_not_switch() {
        let cases = [
            (idle_but(|at| at.starting = true), Refusal::Starting),
            (idle_but(|at| at.turn = true), Refusal::Turn),
            (idle_but(|at| at.card = true), Refusal::Card),
            (idle_but(|at| at.interrupting = true), Refusal::Interrupting),
            (
                idle_but(|at| at.mode_changing = true),
                Refusal::ModeChanging,
            ),
            (idle_but(|at| at.queued = 2), Refusal::Queued(2)),
            (idle_but(|at| at.spoke = false), Refusal::Unspoken),
            (idle_but(|at| at.moved = true), Refusal::Moved),
            (idle_but(|at| at.held = true), Refusal::Held),
            // A card waits on a stopped session's stand-in too: it is answered first.
            (
                idle_but(|at| (at.runs, at.card) = (false, true)),
                Refusal::Card,
            ),
            // Queued messages would be lost to the TUI, stopped or not.
            (
                idle_but(|at| (at.runs, at.queued) = (false, 1)),
                Refusal::Queued(1),
            ),
        ];
        for (at, refusal) in cases {
            let mut switches = Switches::default();
            assert_eq!(switches.hand_to_tui(1, &at), Err(refusal), "{at:?}");
            assert!(!switches.under_way(1), "{at:?}");
        }
    }

    #[test]
    fn each_refusal_names_the_session() {
        let refusals = [
            Refusal::Already,
            Refusal::Starting,
            Refusal::Turn,
            Refusal::Card,
            Refusal::Interrupting,
            Refusal::ModeChanging,
            Refusal::Queued(1),
            Refusal::Queued(3),
            Refusal::Unspoken,
            Refusal::Moved,
            Refusal::Held,
        ];
        for refusal in refusals {
            assert!(refusal.message("api").contains("api"), "{refusal:?}");
        }
        assert!(
            Refusal::Queued(3)
                .message("api")
                .contains("3 queued messages")
        );
        assert!(
            Refusal::Queued(1)
                .message("api")
                .contains("a queued message")
        );
    }
}
