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
//! - **Back from the TUI** (part 4d-2). A TUI session switches while its screen says it is
//!   idle, done or ready for review, or while it is stopped; not while it works or a dialog
//!   asks, nor while its state is not known yet, as a TUI just started is, nor while a resume
//!   is held. Its `claude` is hung up first, and the native resume waits until that process
//!   is gone.
//! - **A TUI the hand-off started** that exits cleanly of itself, as `/exit` ends it, comes
//!   back natively at once (the author's Q4-6 (a)): not one stopped by oso or the person, nor
//!   one that failed.
//! - **One at a time.** A session switches once until its switch is over: the other mode
//!   started, or did not.

use std::collections::{HashMap, HashSet};

use super::super::state::State;

/// Which way a session switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// From native to the TUI (part 4d-1).
    ToTui,
    /// From the TUI back to native (part 4d-2).
    ToNative,
}

/// Where a TUI session is, as the caller knows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TuiAt {
    /// Its `claude` runs, or is being started.
    pub runs: bool,
    /// It is being started.
    pub starting: bool,
    /// What its screen says, once the tracker judged it.
    pub state: Option<State>,
    /// Its `claude`'s process id, once the start thread had it.
    pub pid: Option<u32>,
    /// A resume of it is held now: a stop asked, oso waiting to quit or restarting, or an
    /// orchestrator being replaced.
    pub held: bool,
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
    /// The TUI's screen says it is not idle.
    Busy(State),
    /// The TUI's state is not known yet.
    Unknown,
    /// The TUI's process id is not known, so its end could not be waited for.
    NoProcess,
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
            Self::Busy(state) => {
                format!("Session {name} is {state}: it switches once its screen says idle")
            }
            Self::Unknown => format!(
                "Session {name}'s state is not known yet: it switches once its screen says idle"
            ),
            Self::NoProcess => format!(
                "Session {name}'s claude has no known process, so its end cannot be waited \
                 for: stop it, then switch"
            ),
        }
    }
}

/// What the caller does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    /// End the process, and say when it ended and was reaped ([`Switches::ended`]).
    End,
    /// No process runs: start the other mode, at once to the TUI, after the event to
    /// native ([`Switches::take_due`]).
    Start,
}

/// Where a switch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The process of the mode the session was in is being ended.
    Ending,
    /// It ended, and the other mode starts after the event (a TUI's terminal says its end
    /// while it is being dropped).
    Due,
    /// The other mode is being started.
    Starting,
}

/// The sessions switching, by the caller's key.
#[derive(Debug, Default)]
pub struct Switches {
    map: HashMap<u64, (Way, Phase)>,
    /// The TUI sessions a hand-off started, while their `claude` runs (Q4-6 (a)).
    handed: HashSet<u64>,
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

    /// `session-switch-mode` for the TUI session `key`, which is `at`: back to native,
    /// hanging up its `claude` first if it runs. Whether the project is trusted for a native
    /// session is the caller's to judge first.
    ///
    /// # Errors
    ///
    /// Why it does not switch now.
    pub fn hand_to_native(&mut self, key: u64, at: &TuiAt) -> Result<Then, Refusal> {
        let refusal = if self.map.contains_key(&key) {
            Some(Refusal::Already)
        } else if at.held {
            Some(Refusal::Held)
        } else if at.starting {
            Some(Refusal::Starting)
        } else if !at.runs {
            None
        } else if at.pid.is_none() {
            Some(Refusal::NoProcess)
        } else {
            match at.state {
                None => Some(Refusal::Unknown),
                Some(State::Idle | State::Done | State::ReadyForReview) => None,
                Some(state) => Some(Refusal::Busy(state)),
            }
        };
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        self.handed.remove(&key);
        Ok(self.begin(key, Way::ToNative, at.runs))
    }

    /// The TUI session `key` was started by a hand-off from native.
    pub fn handed(&mut self, key: u64) {
        self.handed.insert(key);
    }

    /// The TUI session `key`'s `claude` ended, of itself with status 0 if `clean`: whether it
    /// comes back natively at once, as a TUI a hand-off started does (Q4-6 (a)). The caller
    /// gives `clean` false for an end that oso or the person caused, or that failed. Either
    /// way the TUI is no longer one a hand-off started; a session that comes back is
    /// switching, its process ended, its native start due.
    pub fn tui_exited(&mut self, key: u64, clean: bool) -> bool {
        let handed = self.handed.remove(&key);
        if !handed || !clean || self.map.contains_key(&key) {
            return false;
        }
        self.map.insert(key, (Way::ToNative, Phase::Due));
        true
    }

    /// The switch begins: its process ends first if it `runs`.
    fn begin(&mut self, key: u64, way: Way, runs: bool) -> Then {
        let (phase, then) = match (runs, way) {
            (true, _) => (Phase::Ending, Then::End),
            (false, Way::ToTui) => (Phase::Starting, Then::Start),
            (false, Way::ToNative) => (Phase::Due, Then::Start),
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

    /// Session `key`'s process ended: if a switch ended it, the other mode starts, the way
    /// this gives: the TUI now, native after the event ([`Switches::take_due`]).
    pub fn ended(&mut self, key: u64) -> Option<Way> {
        let (way, phase) = self.map.get_mut(&key)?;
        if *phase != Phase::Ending {
            return None;
        }
        *phase = match way {
            Way::ToTui => Phase::Starting,
            Way::ToNative => Phase::Due,
        };
        Some(*way)
    }

    /// The sessions whose native start is due, now being started, oldest key first.
    pub fn take_due(&mut self) -> Vec<u64> {
        let mut due: Vec<u64> = (self.map.iter_mut())
            .filter(|(_, (_, phase))| *phase == Phase::Due)
            .map(|(key, (_, phase))| {
                *phase = Phase::Starting;
                *key
            })
            .collect();
        due.sort_unstable();
        due
    }

    /// Whether any session is switching.
    pub fn any(&self) -> bool {
        !self.map.is_empty()
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

    /// Session `key` is gone: nothing of it is kept.
    pub fn forget(&mut self, key: u64) {
        self.map.remove(&key);
        self.handed.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeAt, Refusal, Switches, Then, TuiAt, Way};
    use crate::sessions::state::{Reason, State};

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

    /// A TUI session whose screen says `state`, its `claude` running as process 42.
    fn tui(state: Option<State>) -> TuiAt {
        TuiAt {
            runs: true,
            state,
            pid: Some(42),
            ..TuiAt::default()
        }
    }

    #[test]
    fn an_idle_tui_is_hung_up_then_taken_back_natively() {
        for state in [State::Idle, State::Done, State::ReadyForReview] {
            let mut switches = Switches::default();
            assert_eq!(switches.hand_to_native(1, &tui(Some(state))), Ok(Then::End));
            assert_eq!(switches.ending(1), Some(Way::ToNative));
            assert_eq!(
                switches.hand_to_native(1, &tui(Some(state))),
                Err(Refusal::Already)
            );
            assert_eq!(switches.ended(1), Some(Way::ToNative));
            assert_eq!(switches.starting(1), None, "due after the event");
            assert_eq!(switches.take_due(), [1]);
            assert_eq!(switches.starting(1), Some(Way::ToNative));
            assert!(switches.take_due().is_empty());
            assert_eq!(switches.over(1), Some(Way::ToNative));
            assert!(!switches.any());
        }
        // A stopped one, whatever its last state, at once.
        let mut switches = Switches::default();
        let stopped = TuiAt {
            runs: false,
            pid: None,
            ..tui(Some(State::Working))
        };
        assert_eq!(switches.hand_to_native(2, &stopped), Ok(Then::Start));
        assert_eq!(switches.take_due(), [2]);
        assert_eq!(switches.starting(2), Some(Way::ToNative));
    }

    #[test]
    fn a_tui_that_is_not_idle_does_not_switch() {
        let cases = [
            (tui(Some(State::Working)), Refusal::Busy(State::Working)),
            (
                tui(Some(State::NeedsInput(Reason::Permission))),
                Refusal::Busy(State::NeedsInput(Reason::Permission)),
            ),
            (tui(None), Refusal::Unknown),
            (
                TuiAt {
                    pid: None,
                    ..tui(Some(State::Idle))
                },
                Refusal::NoProcess,
            ),
            (
                TuiAt {
                    starting: true,
                    ..tui(Some(State::Idle))
                },
                Refusal::Starting,
            ),
            (
                TuiAt {
                    held: true,
                    runs: false,
                    ..tui(None)
                },
                Refusal::Held,
            ),
        ];
        for (at, refusal) in cases {
            let mut switches = Switches::default();
            assert_eq!(switches.hand_to_native(1, &at), Err(refusal), "{at:?}");
            assert!(!switches.under_way(1), "{at:?}");
            assert!(refusal.message("api").contains("api"));
        }
    }

    #[test]
    fn a_tui_the_hand_off_started_comes_back_when_it_exits_cleanly_of_itself() {
        let mut switches = Switches::default();
        switches.handed(1);
        assert!(switches.tui_exited(1, true));
        assert_eq!(switches.take_due(), [1]);
        assert_eq!(switches.starting(1), Some(Way::ToNative));
        switches.over(1);
        // Once only: a later exit of a TUI resumed by hand is a TUI session's own.
        assert!(!switches.tui_exited(1, true));
        // Stopped, failed or never handed: it stays a TUI session.
        switches.handed(2);
        assert!(!switches.tui_exited(2, false));
        assert!(!switches.tui_exited(2, true));
        assert!(!switches.tui_exited(3, true));
        // Switched back by the person meanwhile: that switch goes on, not a second one.
        switches.handed(4);
        assert_eq!(
            switches.hand_to_native(4, &tui(Some(State::Idle))),
            Ok(Then::End)
        );
        assert!(!switches.tui_exited(4, true));
        assert_eq!(switches.ending(4), Some(Way::ToNative));
        // A session gone keeps nothing.
        switches.handed(5);
        switches.forget(5);
        assert!(!switches.tui_exited(5, true));
    }
}
