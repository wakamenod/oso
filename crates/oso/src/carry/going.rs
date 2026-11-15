//! The instance going, in `oso-restart` (ADR 0133): on a thread of its own, it checks the
//! binary, starts it with one end of a socket pair, and plays `oso dev`'s supervisor between
//! its own UI thread and the new instance. Its own state it asks of its UI thread with the
//! same [`Op`]s the dev socket brings; the new instance it asks over the [`Pair`].
//!
//! Must not depend on: `app`, `dev`, processes or sockets; `start` starts the new instance
//! and reaches it over the pair, and gives what the UI thread answers through [`Here`].
//!
//! The order, in [`hand_over`], can be tested without processes:
//! 1. The new instance says [`HELLO`] within [`START_TIME`], naming the state versions it
//!    reads, which must hold this one's; otherwise it is stopped.
//! 2. The UI thread exports the state, the terminals stopping their reads; its pieces go over
//!    the pair, then the state, which the new instance answers once its files are judged.
//!    Any failure until then stops the new instance, and this one reads its terminals again.
//! 3. The UI thread hands over a second descriptor of each master, and they go over the pair,
//!    for the new instance to hold until this one has gone.
//! 4. The UI thread exits if nothing changed since the export, letting the terminals go,
//!    nothing hung up. Otherwise, and after any failure since the import, the new instance is
//!    told to [`STAY`] out of it: it gives the terminals up, and closes unless it was edited.
//!
//! So only one instance reads each PTY at a time: this one until the export, the new one only
//! once this one has gone. `dev/restart.rs` is the same order for `oso dev`'s supervisor: a
//! change to one is a change to the other.

// The restart needs the pair and terminals, which only Unix has for now; its parts are
// tested everywhere.
#![cfg_attr(not(unix), allow(dead_code, reason = "oso-restart runs on Unix only"))]

use std::time::Duration;

use serde_json::{Value, json};

use super::fds::Fd;
use super::pair::{STAGE_PIECE, TAKE_MASTERS, TAKE_STATE};
use super::{Op, STATE_VERSION};

/// How long the new instance may take to start and say [`HELLO`].
pub const START_TIME: Duration = Duration::from_secs(10);

/// How long the new instance may take to answer an ask.
pub const ANSWER_TIME: Duration = Duration::from_secs(10);

/// How long the new instance may take to say it holds the masters.
pub const HOLD_TIME: Duration = Duration::from_secs(5);

/// How long this instance's own UI thread may take to answer: an export waits up to 8 s for
/// its terminals.
const UI_TIME: Duration = Duration::from_secs(20);

/// The argument that tells a new instance to take over through the pair it inherits.
pub const TAKE_OVER: &str = "--take-over";

/// What the new instance said first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// The oldest and newest state versions it reads.
    pub versions: (u64, u64),
    /// The terminal library it reads terminals with.
    pub library: Option<String>,
}

/// This instance's own UI thread, as the restart asks it.
pub trait Here {
    /// Asks `op` as the parent does, and returns the answer and the descriptors it put.
    fn ask(&mut self, op: Op) -> Result<(Value, Vec<Fd>), String>;
    /// Asks it to exit if its generation is still `generation`, letting the terminals go.
    /// `Ok` once it has agreed: it then exits.
    fn exit(&mut self, generation: u64) -> Result<(), String>;
    /// The restart is over and this instance stays: its terminals read again, the agent
    /// channel comes back if it was on, and `why` is said.
    fn stays(&mut self, why: String);
}

/// The new instance, over the pair.
pub trait There {
    /// What it said first.
    fn hello(&mut self) -> Result<Hello, String>;
    /// Asks it `say` with `fields` and `fds`, and returns its answer.
    fn ask(
        &mut self,
        say: &str,
        fields: Value,
        fds: Vec<Fd>,
        patience: Duration,
    ) -> Result<Value, String>;
    /// Tells it this instance stays, keeping its terminals.
    fn stay(&mut self);
    /// Stops it, as it holds nothing yet.
    fn stop(&mut self);
}

/// How a restart ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// This instance agreed to exit: the new one has everything.
    Replaced,
    /// The new instance was stopped before it took anything, and this one stays.
    Stopped(String),
    /// The new instance took the state but this one stays, so it was told to [`STAY`].
    Stayed(String),
}

/// Hands this instance's state, texts and terminals to the new instance, in the order the
/// module says, and tells `here` if it stays.
pub fn hand_over(here: &mut impl Here, there: &mut impl There) -> Ended {
    let ended = order(here, there);
    match &ended {
        Ended::Replaced => {}
        Ended::Stopped(why) => {
            there.stop();
            here.stays(why.clone());
        }
        Ended::Stayed(why) => {
            there.stay();
            here.stays(why.clone());
        }
    }
    ended
}

fn order(here: &mut impl Here, there: &mut impl There) -> Ended {
    let stopped = |why: String| Ended::Stopped(format!("the new oso did not take over: {why}"));
    let hello = match there.hello() {
        Ok(hello) => hello,
        Err(why) => return stopped(why),
    };
    if !(hello.versions.0..=hello.versions.1).contains(&STATE_VERSION) {
        return stopped(format!(
            "it reads states {} to {}, and this one writes {STATE_VERSION}",
            hello.versions.0, hello.versions.1
        ));
    }
    let (state, _) = match here.ask(Op::ExportState {
        library: Some(hello.library.unwrap_or_default()),
    }) {
        Ok(exported) => exported,
        Err(why) => return Ended::Stopped(format!("cannot restart: {why}")),
    };
    let Some(generation) = state["generation"].as_u64() else {
        return stopped("the export reported no generation".into());
    };
    let pieces = super::pieces::relay(
        &state,
        |export, item, offset| {
            here.ask(Op::ExportText {
                export,
                item,
                offset,
            })
            .map(|(piece, _)| piece)
        },
        |fields| {
            there
                .ask(STAGE_PIECE, fields, Vec::new(), ANSWER_TIME)
                .map(drop)
        },
    );
    if let Err(why) = pieces {
        return stopped(why);
    }
    if let Err(why) = there.ask(
        TAKE_STATE,
        json!({ "state": state }),
        Vec::new(),
        ANSWER_TIME,
    ) {
        return stopped(format!("it refused the state: {why}"));
    }
    let terminals = state["terminals"].as_array().map_or(0, Vec::len);
    if terminals > 0 {
        let masters = match here.ask(Op::HandOverTerminals { generation }) {
            Ok((_, masters)) if masters.len() == terminals => masters,
            Ok((_, masters)) => {
                return Ended::Stayed(format!(
                    "{} masters were handed over for {terminals} terminals",
                    masters.len()
                ));
            }
            Err(why) => return Ended::Stayed(why),
        };
        let fields = json!({ "count": terminals });
        if let Err(why) = there.ask(TAKE_MASTERS, fields, masters, HOLD_TIME) {
            return Ended::Stayed(format!("the new oso did not take the terminals: {why}"));
        }
    }
    match here.exit(generation) {
        Ok(()) => Ended::Replaced,
        Err(why) => Ended::Stayed(why),
    }
}

mod start;
#[cfg(test)]
use start::inherited_names;
pub use start::{Start, spawn};

#[cfg(test)]
mod tests;
