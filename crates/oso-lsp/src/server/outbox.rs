//! What a session thread reports to the UI, and when it wakes it (ADR 0138, Bounds): the
//! server's state, lines for `*LSP*`, messages for the person and diagnostics, kept in a box
//! the UI takes whole. A change of state and a message wake the UI at once; log lines,
//! progress and diagnostics wake it at most once each [`REPORT_INTERVAL`], the diagnostics
//! coalesced to the latest of each document. What waits is bounded, the oldest lines going
//! first, so a UI that does not take it holds at most [`REPORT_LOG_BYTES_MAX`].
//!
//! Must not depend on: `session`'s loop or `pipes`; this holds what they report.

use std::collections::VecDeque;
use std::mem;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::limits::{
    REPORT_INTERVAL, REPORT_LOG_BYTES_MAX, REPORT_SHOWN_MAX, SHOW_MESSAGE_INTERVAL,
};

use super::{Answer, Closed, Diagnosed, Report, Severity, Shown, State, Wake};

/// The box the session thread fills and the UI empties.
#[derive(Debug, Default)]
pub(super) struct Outbox(Mutex<Waiting>);

#[derive(Debug)]
struct Waiting {
    state: State,
    /// Something new since the UI last took the box.
    fresh: bool,
    log: VecDeque<String>,
    log_bytes: usize,
    /// Lines that went for the bound before the UI took them.
    log_dropped: usize,
    shown: Vec<Shown>,
    documents: Vec<(u64, Option<Closed>)>,
    diagnostics: Vec<Diagnosed>,
    answers: Vec<Answer>,
}

impl Default for Waiting {
    fn default() -> Self {
        Self {
            state: State::Starting,
            fresh: false,
            log: VecDeque::new(),
            log_bytes: 0,
            log_dropped: 0,
            shown: Vec::new(),
            documents: Vec::new(),
            diagnostics: Vec::new(),
            answers: Vec::new(),
        }
    }
}

impl Outbox {
    /// What waits, `None` if nothing new.
    pub(super) fn take(&self) -> Option<Report> {
        let mut waiting = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !mem::take(&mut waiting.fresh) {
            return None;
        }
        let mut log: Vec<String> = Vec::with_capacity(waiting.log.len() + 1);
        let dropped = mem::take(&mut waiting.log_dropped);
        if dropped > 0 {
            log.push(format!(
                "({dropped} lines dropped, past {REPORT_LOG_BYTES_MAX} bytes)"
            ));
        }
        log.extend(waiting.log.drain(..));
        waiting.log_bytes = 0;
        Some(Report {
            state: waiting.state,
            log,
            shown: mem::take(&mut waiting.shown),
            documents: mem::take(&mut waiting.documents),
            diagnostics: mem::take(&mut waiting.diagnostics),
            answers: mem::take(&mut waiting.answers),
        })
    }

    fn with<T>(&self, change: impl FnOnce(&mut Waiting) -> T) -> T {
        let mut waiting = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        waiting.fresh = true;
        change(&mut waiting)
    }
}

/// The session thread's side: what it reports, and when the UI is woken for it.
pub(super) struct Reporter {
    outbox: Arc<Outbox>,
    wake: Wake,
    state: State,
    /// When the UI was last woken.
    woken: Option<Instant>,
    /// Something waits that has not woken the UI yet.
    due: bool,
    /// When a message last reached the person; the rest within the interval go to the log.
    shown_at: Option<Instant>,
}

impl Reporter {
    pub(super) fn new(outbox: Arc<Outbox>, wake: Wake) -> Self {
        Self {
            outbox,
            wake,
            state: State::Starting,
            woken: None,
            due: false,
            shown_at: None,
        }
    }

    /// The server's state now. A start, a stop or an end wakes the UI at once; work beginning,
    /// ending or going on waits for the interval, as a server may begin and end work in bursts.
    pub(super) fn state(&mut self, state: State) {
        if state == self.state {
            return;
        }
        let running = |state: State| matches!(state, State::Ready | State::Working(_));
        let urgent = !(running(state) && running(self.state));
        self.state = state;
        self.outbox.with(|waiting| waiting.state = state);
        if urgent {
            self.wake_now();
        } else {
            self.due = true;
        }
    }

    /// A line for `*LSP*`.
    pub(super) fn log(&mut self, line: String) {
        self.outbox.with(|waiting| {
            waiting.log_bytes += line.len() + 1;
            waiting.log.push_back(line);
            while waiting.log_bytes > REPORT_LOG_BYTES_MAX {
                let Some(oldest) = waiting.log.pop_front() else {
                    break;
                };
                waiting.log_bytes -= oldest.len() + 1;
                waiting.log_dropped += 1;
            }
        });
        self.due = true;
    }

    /// A message for the person, which wakes the UI at once. A server's own (`showMessage`)
    /// reaches the person at most once each [`SHOW_MESSAGE_INTERVAL`], the rest going to the
    /// log only; oso's own, such as why the server stopped, always does.
    pub(super) fn show(
        &mut self,
        severity: Severity,
        text: String,
        from_server: bool,
        now: Instant,
    ) {
        if from_server
            && self
                .shown_at
                .is_some_and(|at| now.duration_since(at) < SHOW_MESSAGE_INTERVAL)
        {
            return;
        }
        if from_server {
            self.shown_at = Some(now);
        }
        self.outbox.with(|waiting| {
            if waiting.shown.len() < REPORT_SHOWN_MAX {
                waiting.shown.push(Shown { severity, text });
            }
        });
        self.wake_now();
    }

    /// Document `id` was kept from the server, and why, or opened with it again (`None`). The
    /// UI is woken at once: the modeline follows it.
    pub(super) fn document(&mut self, id: u64, closed: Option<Closed>) {
        self.outbox.with(|waiting| {
            // One entry per document at most: the latest.
            waiting.documents.retain(|(kept, _)| *kept != id);
            waiting.documents.push((id, closed));
        });
        self.wake_now();
    }

    /// A document's diagnostics, in place of any of its own that wait: the UI is woken within
    /// the interval.
    pub(super) fn diagnostics(&mut self, diagnosed: Diagnosed) {
        let Diagnosed::Open { id, .. } = &diagnosed;
        let id = *id;
        self.outbox.with(|waiting| {
            waiting
                .diagnostics
                .retain(|Diagnosed::Open { id: kept, .. }| *kept != id);
            waiting.diagnostics.push(diagnosed);
        });
        self.due = true;
    }

    /// An answer to an ask, which wakes the UI at once: a person waits for it. At most one of
    /// each kind waits for each document, the latest, as a newer ask makes the older moot; the
    /// older is dropped once the lock is let go, as it can hold 1000 paths.
    pub(super) fn answer(&mut self, answer: Answer) {
        let older = self.outbox.with(|waiting| {
            let key = (answer.document, answer.kind);
            let older = waiting
                .answers
                .iter()
                .position(|kept| (kept.document, kept.kind) == key)
                .map(|at| waiting.answers.swap_remove(at));
            waiting.answers.push(answer);
            older
        });
        drop(older);
        self.wake_now();
    }

    /// Wakes the UI if something waits and the interval has passed. Returns how long until it
    /// may, if something still waits.
    pub(super) fn flush(&mut self, now: Instant) -> Option<Duration> {
        if !self.due {
            return None;
        }
        match self.woken {
            Some(at) if now.duration_since(at) < REPORT_INTERVAL => {
                Some(REPORT_INTERVAL - now.duration_since(at))
            }
            _ => {
                self.wake_now();
                None
            }
        }
    }

    fn wake_now(&mut self) {
        self.due = false;
        self.woken = Some(Instant::now());
        (self.wake)();
    }
}
