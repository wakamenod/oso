//! What holds an `oso-restart` back of the native sessions (ADR 0165, part 4b): a session
//! whose process is starting, that is being interrupted, that has a turn under way, or that
//! asks the person (a permission, question or plan card), or that oso is stopping, as the
//! person or an orchestrator's replacement asked (part 4c: so that the restart neither
//! resumes it nor starts its successor part way). The restart waits for them, as an
//! exit that waits does for a turn (ADR 0133, step 5), and ends their processes only once
//! they are idle. A stopped or failed session, with no process, holds nothing. So does one
//! whose process is being ended for a switch to the TUI (part 4d), until it ended and the
//! TUI's start, which holds it as any start does, began. A composer's send that waits for a
//! dictation's fix holds it too (ADR 0175, step 2).
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::collections::HashSet;

use oso_core::buffer::BufferId;

use super::Native;
use crate::app::App;

/// What a native session is doing, as far as a restart is concerned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Doing {
    /// Its process runs, or is being started: an ended session's turn and state are what its
    /// stream said last.
    live: bool,
    /// Its process is being started.
    starting: bool,
    /// It is being interrupted (step 2c): its process may end and resume.
    interrupting: bool,
    /// A turn is under way, as its stream says.
    turn: bool,
    /// The sessions' worker found it working or asking the person (ADR 0133's busy).
    busy: bool,
    /// oso hung it up, and its end has not come yet (part 4c).
    stopping: bool,
    /// Its process is being ended for a switch to the TUI (part 4d), which starts once it
    /// ended: the restart carries the TUI, not a session caught half way.
    switching: bool,
    /// The person's send waits for a dictation's fix (ADR 0175, step 2).
    sending: bool,
}

impl Doing {
    fn of(native: &Native, busy: bool, stopping: bool, switching: bool) -> Self {
        let starting = native.starting();
        Self {
            live: native.stream.is_some() || starting,
            starting,
            interrupting: native.interrupt.interrupting(),
            turn: native.composing.busy,
            busy,
            stopping,
            switching,
            sending: native.composing.holds_a_send(),
        }
    }

    /// Whether it holds a restart back.
    fn holds(self) -> bool {
        self.live
            && (self.starting
                || self.interrupting
                || self.turn
                || self.busy
                || self.stopping
                || self.switching
                || self.sending)
    }
}

impl App {
    /// Whether any native session holds an `oso-restart` back, with nothing allocated: for
    /// the look before each wait of the loop, while a restart waits. O(buffers + sessions).
    pub(in crate::app) fn natives_hold_restart(&self) -> bool {
        // A busy buffer runs (`Buffer::is_busy`), so it is a live session's.
        let switches = &self.hosting.natives.switches;
        let stopping = |key| self.watch.is_hung_up(key);
        // A TUI being taken back natively holds it too (part 4d-2).
        (self.hosting.natives.map.iter()).any(|(key, native)| {
            Doing::of(native, false, stopping(*key), switches.under_way(*key)).holds()
        }) || switches.any()
            || self.editor.busy_native_sessions().next().is_some()
    }

    /// The buffers of the native sessions an `oso-restart` waits for, in the order they were
    /// made, for the echo area. O(buffers + sessions).
    pub(in crate::app) fn natives_holding_restart(&self) -> Vec<BufferId> {
        if self.hosting.natives.map.is_empty() {
            return Vec::new();
        }
        let busy: HashSet<BufferId> = self.editor.busy_native_sessions().collect();
        let switches = &self.hosting.natives.switches;
        let mut holding: Vec<BufferId> = (self.hosting.natives.map.iter())
            .filter(|(key, native)| {
                let (stopping, switching) =
                    (self.watch.is_hung_up(**key), switches.under_way(**key));
                Doing::of(native, busy.contains(&native.buffer), stopping, switching).holds()
            })
            .map(|(_, native)| native.buffer)
            .collect();
        // Ids are given in the order buffers are made.
        holding.sort_unstable();
        holding
    }
}

#[cfg(test)]
mod tests {
    use super::Doing;

    const LIVE: Doing = Doing {
        live: true,
        starting: false,
        interrupting: false,
        turn: false,
        busy: false,
        stopping: false,
        switching: false,
        sending: false,
    };

    #[test]
    fn a_live_session_starting_interrupted_in_a_turn_or_asking_holds_a_restart() {
        assert!(!LIVE.holds(), "an idle session holds nothing");
        let holding = [
            Doing {
                starting: true,
                ..LIVE
            },
            Doing {
                interrupting: true,
                ..LIVE
            },
            Doing { turn: true, ..LIVE },
            // A card waits for the person: the worker finds it asking.
            Doing { busy: true, ..LIVE },
            // Hung up, until it has ended (part 4c).
            Doing {
                stopping: true,
                ..LIVE
            },
            // The person's send waits for a dictation's fix (ADR 0175).
            Doing {
                sending: true,
                ..LIVE
            },
            // Ended for a switch to the TUI, which is not started yet (part 4d).
            Doing {
                switching: true,
                ..LIVE
            },
        ];
        for doing in holding {
            assert!(doing.holds(), "{doing:?}");
            // Ended, what its stream said last holds nothing.
            let ended = Doing {
                live: false,
                ..doing
            };
            assert!(!ended.holds(), "{ended:?}");
        }
    }
}
