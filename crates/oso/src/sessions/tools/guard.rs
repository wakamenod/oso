//! The guard of the session tools: the person's 60 s refusal of the same call from the same
//! session (ADR 0121, step 6), and each session's rate of calls that ask or change
//! (ADR 0178). Both are kept by session, never by connection.
//!
//! Must not depend on: `app`, `dispatch`, threads or the disk; the guard takes the time it is
//! given.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::sessions::identity::SessionId;

/// How long the person's `n` refuses the same call from the same session without asking.
pub const REFUSED_FOR: Duration = Duration::from_secs(60);
/// The most refusals kept; the oldest goes first.
const MAX_REFUSALS: usize = 256;
/// The most calls that ask or change one session makes in any [`RATE_WINDOW`] (ADR 0178): a
/// bound on a runaway loop, above what an orchestrator's busiest minute takes.
pub const CALLS_A_MINUTE: usize = 30;
/// The sliding window [`CALLS_A_MINUTE`] counts in.
pub const RATE_WINDOW: Duration = Duration::from_secs(60);
/// The sessions whose recent calls are kept before those with none in the window go.
const MAX_RATED: usize = 256;

/// Why the guard refuses a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The person refused the same call from this session less than [`REFUSED_FOR`] ago.
    Person,
    /// The session made [`CALLS_A_MINUTE`] calls in the last [`RATE_WINDOW`]; the next one
    /// may come in this many seconds.
    Rate { wait_secs: u64 },
}

impl Refusal {
    /// What the agent is told.
    pub fn why(self) -> String {
        match self {
            Self::Person => {
                "the person refused this a moment ago; it is their decision: do not try it another way"
                    .to_owned()
            }
            Self::Rate { wait_secs } => format!(
                "this session made {CALLS_A_MINUTE} calls that ask or change in the last minute; \
                 wait {wait_secs} s and try again; do not reach its effect another way meanwhile"
            ),
        }
    }
}

/// The person's refusals, and each session's recent calls for its rate, by session.
#[derive(Debug, Default)]
pub struct Guard {
    refused: HashMap<(SessionId, &'static str, String), Instant>,
    /// When each session's admitted calls of the last [`RATE_WINDOW`] came, oldest first; at
    /// most [`CALLS_A_MINUTE`] each.
    recent: HashMap<SessionId, VecDeque<Instant>>,
}

impl Guard {
    /// Lets a call of `tool` on `target` from `session` at `now` through, or says why it is
    /// refused: the person's refusal first, then the session's rate. Only a call let through
    /// counts toward the rate.
    pub fn admit(
        &mut self,
        session: &SessionId,
        tool: &'static str,
        target: &str,
        now: Instant,
    ) -> Result<(), Refusal> {
        let key = (session.clone(), tool, target.to_owned());
        match self.refused.get(&key) {
            Some(&at) if now.saturating_duration_since(at) < REFUSED_FOR => {
                return Err(Refusal::Person);
            }
            Some(_) => {
                self.refused.remove(&key);
            }
            None => {}
        }
        self.rate(session, now)
    }

    /// Counts a call from `session` at `now`, or refuses it with how long to wait.
    fn rate(&mut self, session: &SessionId, now: Instant) -> Result<(), Refusal> {
        let within = |at: &Instant| now.saturating_duration_since(*at) < RATE_WINDOW;
        if self.recent.len() >= MAX_RATED && !self.recent.contains_key(session) {
            self.recent
                .retain(|_, calls| calls.back().is_some_and(within));
        }
        let calls = self.recent.entry(session.clone()).or_default();
        while calls.front().is_some_and(|at| !within(at)) {
            calls.pop_front();
        }
        if calls.len() >= CALLS_A_MINUTE
            && let Some(&oldest) = calls.front()
        {
            let free = RATE_WINDOW.saturating_sub(now.saturating_duration_since(oldest));
            let wait_secs = free.as_secs() + u64::from(free.subsec_nanos() > 0);
            return Err(Refusal::Rate {
                wait_secs: wait_secs.max(1),
            });
        }
        calls.push_back(now);
        Ok(())
    }

    /// Whether the person refused `tool` on `target` from `session` less than
    /// [`REFUSED_FOR`] before `now`: for a target only known once a job has looked, such as
    /// the branch git resolved a name to.
    pub fn was_refused(
        &self,
        session: &SessionId,
        tool: &'static str,
        target: &str,
        now: Instant,
    ) -> bool {
        self.refused
            .get(&(session.clone(), tool, target.to_owned()))
            .is_some_and(|&at| now.saturating_duration_since(at) < REFUSED_FOR)
    }

    /// The person refused `tool` on `target` from `session` at `now`.
    pub fn refused(&mut self, session: &SessionId, tool: &'static str, target: &str, now: Instant) {
        if self.refused.len() >= MAX_REFUSALS {
            self.refused
                .retain(|_, &mut at| now.saturating_duration_since(at) < REFUSED_FOR);
        }
        if self.refused.len() >= MAX_REFUSALS
            && let Some(oldest) = self
                .refused
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(key, _)| key.clone())
        {
            self.refused.remove(&oldest);
        }
        self.refused
            .insert((session.clone(), tool, target.to_owned()), now);
    }
}

#[cfg(test)]
mod tests;
