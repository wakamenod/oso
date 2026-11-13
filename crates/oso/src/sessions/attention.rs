//! When oso asks for the person's attention for a session (ADR 0121, Q10, step 3): a session
//! that enters needs input, failed, ready for review or done while it is not shown in a focused
//! oso window is told about if it is still so 1 s later; several due within 2 s are told as
//! one; and a session not shown that enters needs input or failed blinks the modeline's mark.
//!
//! Must not depend on: `app`, `dispatch` or `config`; this is pure, driven by the
//! instants its caller gives, as the tracker is, so its tests need no clock.
//!
//! `config` reads [`Toggles`] from `[sessions.notify]`, and the app asks the platform for
//! attention with what [`Attention::tick`] gives.
//!
//! Nothing waits while nothing is due: [`Attention::due`] is `None` then, and no timer runs.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the app drives it once sessions are fed (ADR 0121 step 3)"
    )
)]

use std::time::{Duration, Instant};

use oso_platform::notify::Kind;

use super::state::State;
use super::worker::SessionKey;

/// How long a state must hold before it is told about.
pub const DELAY: Duration = Duration::from_secs(1);

/// The window within which states due are told as one.
pub const MERGE: Duration = Duration::from_secs(2);

/// One phase of a blink, dark or lit.
pub const BLINK_PHASE: Duration = Duration::from_millis(300);

/// How many times a mark blinks: dark then lit, each [`BLINK_PHASE`].
pub const BLINKS: u32 = 3;

/// `[sessions.notify]`: which kinds are told about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toggles {
    pub needs_input: bool,
    pub done: bool,
    pub failed: bool,
    pub ready_for_review: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            needs_input: true,
            done: true,
            failed: true,
            ready_for_review: false,
        }
    }
}

impl Toggles {
    /// The kind of `state` if it is told about.
    fn kind(self, state: State) -> Option<Kind> {
        match state {
            State::NeedsInput(_) if self.needs_input => Some(Kind::NeedsInput),
            State::Failed if self.failed => Some(Kind::Failed),
            State::Done if self.done => Some(Kind::Done),
            State::ReadyForReview if self.ready_for_review => Some(Kind::ReadyForReview),
            _ => None,
        }
    }
}

/// Where a session is, as the app sees it when a state is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub state: State,
    /// Shown in the selected window of a focused oso window.
    pub watched: bool,
}

/// What to tell the person: the most urgent kind among those merged, and the echo area's line.
/// The text names sessions and states only, never screen text (ADR 0121).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub kind: Kind,
    pub text: String,
}

/// A state waiting to be told about.
#[derive(Clone, Debug)]
struct Pending {
    key: SessionKey,
    name: String,
    state: State,
    kind: Kind,
    at: Instant,
}

/// The states waiting, those due and held for the merge, and the blink.
#[derive(Debug, Default)]
pub struct Attention {
    toggles: Toggles,
    pending: Vec<Pending>,
    /// Due, to be told when the merge window ends.
    held: Vec<Pending>,
    /// When the last notice went out: the next waits until [`MERGE`] after it.
    told: Option<Instant>,
    /// The blink under way.
    blink: Option<Blink>,
}

/// A blink: when it started, and the phase whose start is due next, from 1 to `2 * BLINKS`.
#[derive(Clone, Copy, Debug)]
struct Blink {
    start: Instant,
    next: u32,
}

impl Attention {
    pub fn new(toggles: Toggles) -> Self {
        Self {
            toggles,
            ..Self::default()
        }
    }

    pub fn set_toggles(&mut self, toggles: Toggles) {
        self.toggles = toggles;
    }

    /// Session `key`, called `name`, entered `state` at `now`; `shown` if it is in a window of
    /// oso's, `watched` if in the selected window of a focused one.
    pub fn changed(
        &mut self,
        key: SessionKey,
        name: &str,
        state: State,
        shown: bool,
        watched: bool,
        now: Instant,
    ) {
        self.pending.retain(|pending| pending.key != key);
        self.held.retain(|pending| pending.key != key);
        if !shown && matches!(state, State::NeedsInput(_) | State::Failed) {
            self.blink = Some(Blink {
                start: now,
                next: 1,
            });
        }
        let Some(kind) = self.toggles.kind(state) else {
            return;
        };
        if watched {
            return;
        }
        self.pending.push(Pending {
            key,
            name: name.to_owned(),
            state,
            kind,
            at: now + DELAY,
        });
    }

    /// Session `key` is gone.
    pub fn forget(&mut self, key: SessionKey) {
        self.pending.retain(|pending| pending.key != key);
        self.held.retain(|pending| pending.key != key);
    }

    /// The next instant something is due: a state's delay, the merge's end or a blink's
    /// phase. `None` while nothing waits.
    pub fn due(&self) -> Option<Instant> {
        let pending = self.pending.iter().map(|pending| pending.at).min();
        let merged = self
            .told
            .filter(|_| !self.held.is_empty())
            .map(|told| told + MERGE);
        let blink = self
            .blink
            .map(|blink| blink.start + BLINK_PHASE * blink.next);
        [pending, merged, blink].into_iter().flatten().min()
    }

    /// Whether the mark is dark at `now`, in a blink's dark phase.
    pub fn dark(&self, now: Instant) -> bool {
        self.blink.is_some_and(|blink| {
            let phase = now.saturating_duration_since(blink.start).as_millis()
                / BLINK_PHASE.as_millis().max(1);
            phase < u128::from(2 * BLINKS) && phase.is_multiple_of(2)
        })
    }

    /// What is due at `now`: states that held for their delay, with `seen` saying where each
    /// session is now (`None` if it is gone), told as one notice at most once per [`MERGE`].
    pub fn tick(
        &mut self,
        now: Instant,
        seen: impl Fn(SessionKey) -> Option<Seen>,
    ) -> Option<Notice> {
        if let Some(blink) = &mut self.blink {
            while blink.next <= 2 * BLINKS && blink.start + BLINK_PHASE * blink.next <= now {
                blink.next += 1;
            }
            if blink.next > 2 * BLINKS {
                self.blink = None;
            }
        }
        let (due, waiting): (Vec<Pending>, Vec<Pending>) = self
            .pending
            .drain(..)
            .partition(|pending| pending.at <= now);
        self.pending = waiting;
        self.held.extend(due);
        // Held ones whose session moved on, was watched or went are dropped, as they wait.
        self.held.retain(|pending| {
            seen(pending.key).is_some_and(|seen| seen.state == pending.state && !seen.watched)
        });
        if self.held.is_empty() || self.told.is_some_and(|told| now < told + MERGE) {
            return None;
        }
        self.told = Some(now);
        Some(notice(&std::mem::take(&mut self.held)))
    }
}

/// One notice for `told`, which is not empty: the most urgent kind, and a line naming the
/// session if there is one, else counting each kind.
fn notice(told: &[Pending]) -> Notice {
    let order = [
        Kind::NeedsInput,
        Kind::Failed,
        Kind::ReadyForReview,
        Kind::Done,
    ];
    let kind = order
        .into_iter()
        .find(|kind| told.iter().any(|pending| pending.kind == *kind))
        .unwrap_or(Kind::Done);
    let text = match told {
        [one] => format!("{} {}", one.name, says(one.kind, 1)),
        _ => order
            .into_iter()
            .filter_map(|kind| {
                let count = told.iter().filter(|pending| pending.kind == kind).count();
                (count > 0).then(|| format!("{count} {}", says(kind, count)))
            })
            .collect::<Vec<_>>()
            .join(", "),
    };
    Notice { kind, text }
}

/// What a notice says of `count` sessions of `kind`, after their name or their count.
fn says(kind: Kind, count: usize) -> &'static str {
    match (kind, count) {
        (Kind::NeedsInput, 1) => "needs input",
        (Kind::NeedsInput, _) => "sessions need input",
        (Kind::Failed, 1) => "failed",
        (Kind::Failed, _) => "sessions failed",
        (Kind::ReadyForReview, 1) => "is ready for review",
        (Kind::ReadyForReview, _) => "sessions are ready for review",
        (Kind::Done, 1) => "is done",
        (Kind::Done, _) => "sessions are done",
    }
}

#[cfg(test)]
mod tests;
