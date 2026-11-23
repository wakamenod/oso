//! What holds an `oso-restart` back of the native sessions (ADR 0165, part 4b): a session
//! whose process is starting, that is being interrupted, that has a turn under way, or that
//! asks the person (a permission, question or plan card). The restart waits for them, as an
//! exit that waits does for a turn (ADR 0133, step 5), and ends their processes only once
//! they are idle. A stopped or failed session, with no process, holds nothing.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::collections::HashSet;

use oso_core::buffer::BufferId;

use super::Native;
use super::note::NativeState;
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
}

impl Doing {
    fn of(native: &Native, busy: bool) -> Self {
        let starting = native.state == NativeState::Starting;
        Self {
            live: native.stream.is_some() || starting,
            starting,
            interrupting: native.interrupt.interrupting(),
            turn: native.composing.busy,
            busy,
        }
    }

    /// Whether it holds a restart back.
    fn holds(self) -> bool {
        self.live && (self.starting || self.interrupting || self.turn || self.busy)
    }
}

impl App {
    /// Whether any native session holds an `oso-restart` back, with nothing allocated: for
    /// the look before each wait of the loop, while a restart waits. O(buffers + sessions).
    pub(in crate::app) fn natives_hold_restart(&self) -> bool {
        // A busy buffer runs (`Buffer::is_busy`), so it is a live session's.
        (self.hosting.natives.map.values()).any(|native| Doing::of(native, false).holds())
            || self.editor.busy_native_sessions().next().is_some()
    }

    /// The buffers of the native sessions an `oso-restart` waits for, in the order they were
    /// made, for the echo area. O(buffers + sessions).
    pub(in crate::app) fn natives_holding_restart(&self) -> Vec<BufferId> {
        if self.hosting.natives.map.is_empty() {
            return Vec::new();
        }
        let busy: HashSet<BufferId> = self.editor.busy_native_sessions().collect();
        let mut holding: Vec<BufferId> = (self.hosting.natives.map.values())
            .filter(|native| Doing::of(native, busy.contains(&native.buffer)).holds())
            .map(|native| native.buffer)
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
