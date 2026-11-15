//! What a restart asks of an instance's UI thread (ADR 0120, step 11; ADR 0133): the same
//! requests whether `oso dev`'s supervisor sends them over the dev socket or the threads of
//! `oso-restart` pass them over the pair, and what comes with each.
//!
//! Must not depend on: `app` or sockets; these are values.

// The restart needs the pair and terminals, which only Unix has for now; its parts are
// tested everywhere.
#![cfg_attr(not(unix), allow(dead_code, reason = "oso-restart runs on Unix only"))]

use super::State;
use super::fds::{Fd, Passing};
use super::pieces::CarriedPiece;

/// A request of a restart, answered on the UI thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Report the generation, or why this instance's text would not survive a restart; with
    /// `terminals`, a running terminal holds nothing. Only the dev socket asks: `oso-restart`
    /// checks on the UI thread itself.
    #[cfg(feature = "dev")]
    CheckHandover { terminals: bool },
    /// Report what a new instance needs to take over; with `terminals`, the name of the
    /// terminal library the new instance reads, its terminals too.
    ExportState { terminals: Option<String> },
    /// Give the masters of the terminals the latest export carries, if the generation is still
    /// this one; the exit that follows lets them go.
    HandOverTerminals { generation: u64 },
    /// Take the masters of the terminals the import carried, in their order; `None` gives them
    /// up, as the instance before kept them.
    ImportTerminals { count: Option<usize> },
    /// Take over from the instance that exported this state.
    ImportState(Box<State>),
    /// Give the piece of an item of the latest export from `offset` (ADR 0090).
    ExportText {
        export: u64,
        item: usize,
        offset: usize,
    },
    /// Stage a piece of an item for the import that follows.
    ImportText(CarriedPiece),
}

/// The descriptors a request brought and those its reply takes, and who asked.
#[derive(Debug, Default)]
pub struct Passed {
    /// What came with the request: only [`Op::ImportTerminals`] takes them, and the rest close
    /// when the request is dropped.
    pub brought: Vec<Fd>,
    /// What goes with the reply, which the UI thread puts.
    pub passing: Passing,
    /// Whether the asker is this instance's parent: `oso dev`'s supervisor, the harness that
    /// started it, or, over the pair, the instance that started it with `oso-restart`. Only it
    /// may ask for terminals.
    pub parent: bool,
}

/// What a thread of `oso-restart` asks of its instance's UI thread (ADR 0133).
#[derive(Debug)]
pub enum Asked {
    /// An [`Op`], asked as the parent does.
    Op {
        op: Op,
        passed: Passed,
        reply: std::sync::mpsc::Sender<super::Reply>,
    },
    /// In the instance going: exit if the generation is still `generation`, letting the
    /// terminals go, nothing hung up.
    Exit {
        generation: u64,
        reply: std::sync::mpsc::Sender<super::Reply>,
    },
    /// In the instance going: the restart is over and it stays. Its terminals read again, the
    /// agent channel comes back if it was on, and the reason is said.
    Stays(String),
    /// In the new instance: the one before stays after all. The terminals it carried are given
    /// up, and it closes if nothing changed since the import, whose generation this is.
    Left { generation: Option<u64> },
}
