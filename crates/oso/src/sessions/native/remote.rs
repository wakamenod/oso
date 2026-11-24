//! Where Remote Control is in a native session's process (ADR 0181): off, connecting, on,
//! dropped or ended, and which of its messages came from elsewhere.
//!
//! Must not depend on: threads, the stream or the UI; the clock and the random numbers are the
//! caller's.
//!
//! - **The start decides off.** A process started with Remote Control blocked
//!   (`sessions::settings`) is off for its life. One started to connect is marked from its start,
//!   asked or not, as it may connect of itself. The stream is not authenticated: a process the
//!   session runs may write any line to it. So no line turns Remote Control on, and none turns it
//!   off: only the process's end clears the mark. An answer refused, or a line that says the
//!   connection is gone, is [`Phase::Dropped`], still marked: what a forged line can do is say
//!   "connected" or "dropped" of a marked process.
//! - **Off is told, and ends the process** (`off`): at once where the session is idle, else once
//!   its turn is over, at most [`off::TURN_WAIT`] later, or at once where claude.ai's side was not
//!   told.
//! - **Back.** A session connected again by `oso-restart` is, only as the session it was there:
//!   an answer that gives another id, or none, turns it off.
//! - Turned off, it is [`Phase::Ended`] until the process ended: no line changes it, and messages
//!   and modes are judged as before.
//! - A request goes only where `initialize` said Remote Control is available. Its id is random,
//!   unlike oso's other requests', so that no line written to the stream can answer it and give an
//!   address of its own. An answer counts only for the request that waits, by its id (`control`),
//!   once; one past its wait counts for nothing.
//! - **The canary.** Off, an `initialize` that says it is available, or a `bridge_state` that says
//!   it connects, means that the block may no longer hold: said once a process. Nothing is turned on,
//!   but from then on messages and modes are judged as in a marked process, which only takes away.
//! - **Messages.** Judged so ([`Remote::watched`]), a message the CLI replays came another way than
//!   oso's standard input if it says an origin, or if its id went through the CLI's queue
//!   (`command_lifecycle`): oso sends its own with no id, and they have neither (measured, CLI
//!   2.1.330). At most [`MAX_COMING`] such ids are kept. Such a message is never the person's
//!   message at oso (`rows`), whatever its text: a forged line only makes one read as less.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::control::Control;
use super::wire::Event;
use super::wire::control::Request;
use super::wire::remote::{
    Asked, Came, Connection, ConnectionState, Enabled, Lifecycle, Origin, RemoteId, Stage,
};
use crate::git::printable;

pub mod off;
pub mod refusal;

use off::Off;

/// How long a request to turn it on has to be answered: an answer took up to 7 s (measured).
pub const ANSWER_WAIT: Duration = Duration::from_secs(30);
/// The most ids of messages that come which are kept, the oldest forgotten first.
const MAX_COMING: usize = 64;
/// Set in the id of a request to turn it on, which is no id of oso's other requests.
const RANDOM_ID: u64 = 1 << 63;

/// The most rows a process's state gives, so that no line floods them: the modeline says the
/// rest.
const MAX_ROWS: usize = 6;
/// The most characters of the CLI's words of why it dropped in a row.
const MAX_WHY: usize = 200;

/// What the canary says.
pub const CANARY: &str = "This claude says that Remote Control, which oso turned off for it, \
     is available or connecting. oso did not ask it to connect.";

/// Where Remote Control is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// The process was started with it blocked.
    #[default]
    Off,
    /// The process was started to connect: not asked, not answered or not connected yet.
    Connecting,
    /// Connected, as the CLI says.
    On,
    /// Refused, not answered, or its connection gone, with the CLI's words of why, shown only
    /// escaped. It may connect again of itself.
    Dropped(String),
    /// Turned off by oso: the process ends.
    Ended,
}

/// Why a request was not begun.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The process was started with it blocked.
    Blocked,
    /// Its control layer does not go, or is not known to yet.
    NoControl,
    /// `initialize` did not say that it is available.
    Unavailable,
    /// It was asked already, or turned off, in this process.
    Already,
}

/// Remote Control in one process; by default in one it is blocked in, or in none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Remote {
    phase: Phase,
    /// `initialize` said that it is available.
    available: bool,
    /// oso asked, in this process.
    begun: bool,
    /// The request to turn it on that waits, to be answered by then.
    waits: Option<(u64, Instant)>,
    /// What its answer gave.
    enabled: Enabled,
    /// The canary said so.
    warned: bool,
    /// The ids of messages that come from elsewhere, oldest first.
    coming: VecDeque<String>,
    /// The session there it comes back to.
    back: Option<RemoteId>,
    /// `initialize` was answered.
    initialized: bool,
    /// How it is turned off.
    off: Option<Off>,
    /// The state the last row said, and how many rows said one.
    row: Option<Phase>,
    rows: usize,
}

impl Remote {
    /// The state of a process started to connect, without the block.
    pub fn unblocked() -> Self {
        Self {
            phase: Phase::Connecting,
            ..Self::default()
        }
    }

    /// That state, of a session that comes back to the session `id` there, if `Some`.
    pub fn coming_back(id: Option<RemoteId>) -> Self {
        Self {
            back: id,
            ..Self::unblocked()
        }
    }

    /// Its start thread blocked it after all: off, as it is told from the process's start; what
    /// the stream said meanwhile is judged so. Whether the canary says so.
    pub fn start_blocked(&mut self) -> bool {
        let available = self.available;
        let connected = self.phase != Phase::Connecting;
        *self = Self {
            available,
            coming: std::mem::take(&mut self.coming),
            ..Self::default()
        };
        (available || connected) && self.canary()
    }

    /// Whether the session is marked, until its process ended: it may be connected.
    pub fn marked(&self) -> bool {
        self.phase != Phase::Off
    }

    /// Whether messages and modes may come from elsewhere: marked, or the canary said so.
    pub fn watched(&self) -> bool {
        self.marked() || self.warned
    }

    /// oso asks, at the person's word alone, at `now`, with the random number `random`: what
    /// the request is written with.
    ///
    /// # Errors
    ///
    /// [`Refusal`]: why it is not asked.
    pub fn begin(
        &mut self,
        control: &mut Control,
        random: u64,
        now: Instant,
    ) -> Result<Asked, Refusal> {
        if !self.marked() {
            return Err(Refusal::Blocked);
        }
        if self.begun || self.phase == Phase::Ended {
            return Err(Refusal::Already);
        }
        if !control.has(Request::RemoteControl) {
            return Err(Refusal::NoControl);
        }
        if !self.available {
            return Err(Refusal::Unavailable);
        }
        let id = random | RANDOM_ID;
        control.ask_as(id, Request::RemoteControl);
        self.begun = true;
        self.waits = Some((id, now + ANSWER_WAIT));
        Ok(Asked::new(id))
    }

    /// oso asks, at `now`, if the process is to be asked and that is known to go, with a random
    /// number of `random`'s: what the request is written with. A process that cannot be asked is
    /// dropped, with why.
    pub fn ready(
        &mut self,
        control: &mut Control,
        random: impl FnOnce() -> std::io::Result<u64>,
        now: Instant,
    ) -> Option<Asked> {
        if self.phase == Phase::Off || self.phase == Phase::Ended || self.begun {
            return None;
        }
        if !self.initialized && control.deadline().is_some() {
            return None;
        }
        let why = match random().map(|random| self.begin(control, random, now)) {
            Ok(Ok(asked)) => return Some(asked),
            Ok(Err(Refusal::Unavailable)) => "this claude says it is not available",
            Ok(Err(_)) => "this claude takes no requests",
            Err(_) => "the system gave no random number",
        };
        self.begun = true;
        self.dropped(why);
        None
    }

    /// Its request was not written.
    pub fn not_written(&mut self, control: &mut Control) {
        if let Some((id, _)) = self.waits.take() {
            control.forget(id);
            self.dropped("its request was not written");
        }
    }

    /// oso turns it off at `now`, its request written with the id given, if it could be; the
    /// process then ends (`off`). `None` where it is blocked, or was turned off already.
    pub fn end(&mut self, control: Option<&mut Control>, now: Instant) -> Option<u64> {
        if matches!(self.phase, Phase::Off | Phase::Ended) {
            return None;
        }
        let waits = self.waits.take();
        self.phase = Phase::Ended;
        let id = control.map(|control| {
            if let Some((id, _)) = waits {
                control.forget(id);
            }
            control.ask(Request::RemoteControl)
        });
        self.off = Some(Off::begun(id, now));
        id
    }

    /// How it is turned off, while it is.
    pub fn off(&self) -> Option<&Off> {
        self.off.as_ref()
    }

    /// Its request that turns it off was not written.
    pub fn off_not_written(&mut self) {
        if let Some(off) = &mut self.off {
            off.not_told();
        }
    }

    /// Where it is.
    #[cfg(test)]
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// What the answer gave.
    pub fn enabled(&self) -> &Enabled {
        &self.enabled
    }

    /// The CLI answered the request `id`, refused with `error` if `Some`: the one that turns it
    /// on, the one that turns it off, or another's, which counts for nothing. Whether the
    /// session there is not the one it comes back to: it is to be turned off.
    pub fn answered(&mut self, id: u64, error: Option<&str>, enabled: Option<Enabled>) -> bool {
        if let Some(off) = &mut self.off {
            off.answered(id, error.is_none());
        }
        if self.waits.take_if(|(waits, _)| *waits == id).is_none() {
            return false;
        }
        match error {
            None => self.enabled = enabled.unwrap_or_default(),
            Some(why) => self.dropped(why),
        }
        error.is_none() && self.back.is_some() && self.back != self.enabled.id
    }

    /// The session there it comes back to.
    pub fn back(&self) -> Option<&RemoteId> {
        self.back.as_ref()
    }

    /// The session there, while the person has it on, as a restart carries it: none once it was
    /// turned off, or where no answer gave one.
    pub fn carried(&self) -> Option<&RemoteId> {
        let on = !matches!(self.phase, Phase::Off | Phase::Ended);
        self.enabled.id.as_ref().filter(|_| on)
    }

    /// Its mark's words, as the modeline says it, while it is marked.
    pub fn mark(&self) -> Option<&'static str> {
        match self.phase {
            Phase::Off => None,
            Phase::Connecting => Some("⇄ connecting"),
            Phase::On => Some("⇄ claude.ai"),
            Phase::Dropped(_) => Some("⇄ dropped"),
            Phase::Ended => Some("⇄ going off"),
        }
    }

    /// The row that says where it is now, if it came there since the last and rows are left.
    pub fn take_row(&mut self) -> Option<String> {
        let row = match &self.phase {
            Phase::On => {
                "Remote Control is on: claude.ai follows this session, sends it messages and stops \
                 it. native-remote-control turns it off; native-remote-control-address copies its \
                 address there."
                    .to_owned()
            }
            Phase::Dropped(why) => format!(
                "Remote Control dropped, and may connect again by itself: {}",
                match why.trim() {
                    "" => "claude gave no reason".to_owned(),
                    why => printable(why, MAX_WHY),
                }
            ),
            Phase::Off | Phase::Connecting | Phase::Ended => return None,
        };
        if self.row.as_ref() == Some(&self.phase) || self.rows == MAX_ROWS {
            return None;
        }
        self.row = Some(self.phase.clone());
        self.rows += 1;
        Some(row)
    }

    /// `initialize`'s first answer says whether Remote Control is available. Whether the canary
    /// says so.
    pub fn initialized(&mut self, available: Option<bool>) -> bool {
        self.available = available == Some(true);
        self.initialized = true;
        self.available && self.canary()
    }

    /// Notes what `event` says, and judges how a message came
    /// ([`User::came`](super::wire::User::came)). Whether the canary says so.
    pub fn event(&mut self, event: &mut Event) -> bool {
        match event {
            Event::Connection(connection) => return self.connection(connection),
            Event::Lifecycle(lifecycle) => self.lifecycle(lifecycle),
            Event::User(user) if self.watched() && user.replay && user.parent.is_none() => {
                let came = user.uuid.as_deref().is_some_and(|uuid| self.came(uuid));
                user.came = match (user.origin, came) {
                    (Origin::Unsaid, false) => Came::AsSaid,
                    (Origin::Human, _) | (Origin::Unsaid, true) => Came::Remote,
                    (Origin::Other, _) => Came::Elsewhere,
                };
            }
            _ => {}
        }
        false
    }

    /// When the request's wait ends, while one waits. When its process is to end is [`Off`]'s.
    pub fn deadline(&self) -> Option<Instant> {
        self.waits.map(|(_, by)| by)
    }

    /// What is due at `now`: a request not answered in time is dropped, and still marked. Its
    /// answer is none any more; a connection that comes is said by `bridge_state`.
    pub fn due(&mut self, control: &mut Control, now: Instant) {
        let Some((id, _)) = self.waits.take_if(|(_, by)| now >= *by) else {
            return;
        };
        control.forget(id);
        // Connected meanwhile, it is on: only its address is not known.
        if self.phase == Phase::Connecting {
            self.dropped("it was not answered");
        }
    }

    fn connection(&mut self, connection: &Connection) -> bool {
        use ConnectionState::{Connected, Failed, PolicyDisabled, Ready, Reconnecting};
        match (&self.phase, connection.state) {
            (Phase::Off, Ready | Connected | Reconnecting) => return self.canary(),
            (Phase::Off | Phase::Ended, _) | (Phase::On, Ready) => {}
            (_, Connected) => self.phase = Phase::On,
            (_, Ready) => self.phase = Phase::Connecting,
            (_, Reconnecting | Failed | PolicyDisabled) => self.dropped(&connection.detail),
        }
        false
    }

    fn dropped(&mut self, why: &str) {
        self.phase = Phase::Dropped(why.to_owned());
    }

    /// The canary, once a process that is off.
    fn canary(&mut self) -> bool {
        self.phase == Phase::Off && !std::mem::replace(&mut self.warned, true)
    }

    fn lifecycle(&mut self, lifecycle: &mut Lifecycle) {
        if !self.watched() {
            return;
        }
        let known = self.coming.iter().position(|id| *id == lifecycle.uuid);
        match (lifecycle.stage, known) {
            (Stage::Comes, None) => {
                if self.coming.len() == MAX_COMING {
                    self.coming.pop_front();
                }
                self.coming.push_back(std::mem::take(&mut lifecycle.uuid));
            }
            (Stage::Over, Some(at)) => {
                self.coming.remove(at);
            }
            (Stage::Comes, Some(_)) | (Stage::Over, None) => {}
        }
    }

    /// Whether the message `uuid` went through the CLI's queue, said once.
    fn came(&mut self, uuid: &str) -> bool {
        let at = self.coming.iter().position(|id| id == uuid);
        at.and_then(|at| self.coming.remove(at)).is_some()
    }
}

#[cfg(test)]
mod asking_tests;
#[cfg(test)]
mod tests;
