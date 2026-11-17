//! A hosted session's state (ADR 0121): the one taxonomy every view shows, and the tracker that
//! turns the rules' verdicts, the process's exit, the person's looks and the review's answers
//! into it, with the debounce and the grace period.
//!
//! Must not depend on: `app` or `dispatch`; the tracker is pure, driven by the
//! instants its caller gives, so its tests need no clock.
//!
//! No state authorises anything: not a paste, not an answer, not an action of an agent.

use std::fmt;
use std::time::{Duration, Instant};

use crate::git::Id;

/// What a dialog that needs the person is (ADR 0121).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reason {
    Permission,
    Question,
    Plan,
    /// An MCP server asks for input.
    Elicitation,
    /// Another known dialog.
    Dialog,
}

impl Reason {
    /// The reason a rules file names, if it is one.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "permission" => Self::Permission,
            "question" => Self::Question,
            "plan" => Self::Plan,
            "elicitation" => Self::Elicitation,
            "dialog" => Self::Dialog,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Plan => "plan",
            Self::Elicitation => "elicitation",
            Self::Dialog => "dialog",
        }
    }
}

/// A session's state, most urgent first (ADR 0121).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// A known dialog is open.
    NeedsInput(Reason),
    /// The process ended with a non-zero status or a signal oso did not send.
    Failed,
    /// A turn is under way.
    Working,
    /// The turn ended, and the directory has changes since the session's baseline that no
    /// review has shown and that are not merged.
    ReadyForReview,
    /// The turn ended, and the person has not seen the session since.
    Done,
    /// The turn ended and has been seen, or the TUI waits at its prompt.
    Idle,
    /// Not running.
    Stopped,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedsInput(reason) => write!(f, "needs input ({})", reason.name()),
            Self::Failed => f.write_str("failed"),
            Self::Working => f.write_str("working"),
            Self::ReadyForReview => f.write_str("ready for review"),
            Self::Done => f.write_str("done"),
            Self::Idle => f.write_str("idle"),
            Self::Stopped => f.write_str("stopped"),
        }
    }
}

/// What a look for a session's changes found after its turn ended (ADR 0121, Q5), before a
/// worktree job judges whether they are merged; the tracker takes the verdict as a `bool`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangesFound {
    /// git failed: the state stays as it is.
    Failed,
    /// The tree is the baseline's.
    None,
    /// The tree differs from the baseline: `tree` is it, and `whole` says git read every file
    /// that it does not ignore.
    Some { tree: Id, whole: bool },
}

/// What the rules said of one look at the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    NeedsInput(Reason),
    Working,
    /// Idle; `visible` if a rule saw a sign of it, which needs no debounce.
    Idle {
        visible: bool,
    },
    /// Keep the last state: a full-screen view of the TUI is open, or echo is off.
    Hold,
}

/// How the process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// Status 0, or stopped by the person or oso.
    Stopped,
    /// A non-zero status, or a signal oso did not send.
    Failed,
}

/// A new state, and since when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub state: State,
    pub since: Instant,
    /// A turn ended: working became needs input, done or idle (ADR 0121, the seam to ADR 0122).
    pub turn_ended: bool,
}

/// No transition but to needs input this long after a session starts (ADR 0121).
pub const GRACE: Duration = Duration::from_secs(3);

/// How far apart the idle verdicts that confirm a turn's end are.
pub const CONFIRM_EVERY: Duration = Duration::from_millis(100);

/// How many idle verdicts confirm it.
const CONFIRMATIONS: u8 = 3;

/// The longest a turn's end without a visible sign waits.
pub const CONFIRM_AT_MOST: Duration = Duration::from_millis(700);

/// A move to idle that waits to be confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    first: Instant,
    last: Instant,
    count: u8,
}

/// One session's state, from what it is told.
#[derive(Debug)]
pub struct Tracker {
    state: State,
    since: Instant,
    started: Instant,
    pending: Option<Pending>,
    /// A verdict the grace period kept out, to try again when it ends.
    held_back: bool,
    /// Its state was told once: a session that starts idle and stays so tells it with the
    /// first idle verdict after its grace, else nothing would say it was judged.
    told: bool,
}

impl Tracker {
    /// A session that starts, or is resumed, at `now`: idle, in its grace period. A start is
    /// never a turn's end.
    pub fn new(now: Instant) -> Self {
        Self {
            state: State::Idle,
            since: now,
            started: now,
            pending: None,
            held_back: false,
            told: false,
        }
    }

    #[cfg(test)]
    pub fn state(&self) -> State {
        self.state
    }

    /// Since when the state holds.
    #[cfg(test)]
    pub fn since(&self) -> Instant {
        self.since
    }

    /// When the screen should be judged again with nothing new on it: to confirm a turn's end,
    /// or once the grace period is over. `None` while nothing waits, so that no timer runs.
    pub fn due(&self) -> Option<Instant> {
        let confirm = self.pending.map(|p| p.last + CONFIRM_EVERY);
        let grace = self.held_back.then_some(self.started + GRACE);
        match (confirm, grace) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Whether the debounce waits: for `session-explain`.
    #[cfg(test)]
    pub fn confirming(&self) -> Option<u8> {
        self.pending.map(|p| p.count)
    }

    /// The rules' verdict on the screen at `now`.
    pub fn verdict(&mut self, target: Target, now: Instant) -> Option<Change> {
        if matches!(self.state, State::Failed | State::Stopped) {
            return None;
        }
        let in_grace = now < self.started + GRACE;
        if in_grace && !matches!(target, Target::NeedsInput(_) | Target::Hold) {
            self.pending = None;
            self.held_back = true;
            return None;
        }
        self.held_back = false;
        match target {
            Target::Hold => {
                self.pending = None;
                None
            }
            Target::NeedsInput(reason) => {
                self.pending = None;
                self.become_(State::NeedsInput(reason), now)
            }
            Target::Working => {
                self.pending = None;
                self.become_(State::Working, now)
            }
            Target::Idle { visible } => self.idle(visible, now),
        }
    }

    /// An idle verdict: at once from a state the turn has left, and from working or a dialog
    /// once confirmed.
    fn idle(&mut self, visible: bool, now: Instant) -> Option<Change> {
        if !matches!(self.state, State::Working | State::NeedsInput(_)) {
            self.pending = None;
            return self.first_word();
        }
        if !visible {
            let pending = self.pending.get_or_insert(Pending {
                first: now,
                last: now,
                count: 0,
            });
            if pending.count == 0 || now >= pending.last + CONFIRM_EVERY {
                pending.count += 1;
                pending.last = now;
            }
            let confirmed =
                pending.count >= CONFIRMATIONS || now >= pending.first + CONFIRM_AT_MOST;
            if !confirmed {
                return None;
            }
        }
        self.pending = None;
        // Only a turn's end makes a session done; a dialog the person answered was seen.
        let next = if self.state == State::Working {
            State::Done
        } else {
            State::Idle
        };
        self.become_(next, now)
    }

    /// The process ended at `now`.
    pub fn exited(&mut self, exit: Exit, now: Instant) -> Option<Change> {
        self.pending = None;
        self.held_back = false;
        let next = match exit {
            Exit::Stopped => State::Stopped,
            Exit::Failed => State::Failed,
        };
        self.become_(next, now)
    }

    /// The person has seen the session: shown in the selected window of a focused oso window
    /// for 2 s. Reading it through a tool does not count.
    pub fn seen(&mut self, now: Instant) -> Option<Change> {
        if self.state == State::Done {
            return self.become_(State::Idle, now);
        }
        None
    }

    /// What `git diff` found after a turn ended: `unmerged` if the directory changed since the
    /// session's baseline (ADR 0121, Q5) and those changes are not merged. Only a session the
    /// turn left done or idle takes it.
    pub fn changes(&mut self, unmerged: bool, now: Instant) -> Option<Change> {
        if unmerged && matches!(self.state, State::Done | State::Idle) {
            return self.become_(State::ReadyForReview, now);
        }
        None
    }

    /// A review of the session was opened, which shows its changes.
    pub fn review_opened(&mut self, now: Instant) -> Option<Change> {
        if self.state == State::ReadyForReview {
            return self.become_(State::Idle, now);
        }
        None
    }

    /// A listing found the session's worktree clean and its branch merged, reading its changes
    /// at `looked_at`: its changes need no review (ADR 0121, "Merged work is not ready for
    /// review, as amended"). A session ready for review only since then keeps it, as its
    /// changes may be newer than what was read.
    pub fn work_merged(&mut self, looked_at: Instant, now: Instant) -> Option<Change> {
        if self.since > looked_at {
            return None;
        }
        self.review_opened(now)
    }

    fn become_(&mut self, next: State, now: Instant) -> Option<Change> {
        if next == self.state {
            return None;
        }
        self.told = true;
        let turn_ended = self.state == State::Working
            && matches!(next, State::NeedsInput(_) | State::Done | State::Idle);
        self.state = next;
        self.since = now;
        Some(Change {
            state: next,
            since: now,
            turn_ended,
        })
    }

    /// The state it started in, as a change, if no change has told one yet: an idle start
    /// is judged too, and its time in the state counts from the start.
    fn first_word(&mut self) -> Option<Change> {
        if std::mem::replace(&mut self.told, true) {
            return None;
        }
        Some(Change {
            state: self.state,
            since: self.since,
            turn_ended: false,
        })
    }
}

#[cfg(test)]
mod tests;
