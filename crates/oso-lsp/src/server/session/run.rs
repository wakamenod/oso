//! One run of a server's process, as its session thread keeps it (ADR 0138, step 3): the
//! writer, the requests in flight and their waits, the progress reported, the flood count, and
//! where a stop the person asked for is.
//!
//! Must not depend on: the outbox or the process itself; the session thread reports and
//! signals.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::limits::{
    EXIT_WAIT, FLOOD_BYTES_MAX, FLOOD_MESSAGES_MAX, FLOOD_WINDOW, PROGRESS_TOKENS_MAX,
    SHUTDOWN_WAIT, WRITER_QUEUE_BYTES_MAX,
};
use crate::protocol::{self, Progress, Token};

use super::super::State;
use super::super::pipes::{Credit, Stderr, Writer};
use super::super::policy::Flood;
use super::{After, Asked, Phase};

/// One run's state.
pub(super) struct Run {
    pub(super) id: u64,
    pub(super) writer: Option<Writer>,
    pub(super) credit: Arc<Credit>,
    pub(super) stderr: Arc<Stderr>,
    pub(super) next_id: i64,
    pub(super) pending: HashMap<i64, (Asked, Instant)>,
    pub(super) started: Instant,
    pub(super) ready: bool,
    pub(super) tokens: HashMap<Token, Option<u8>>,
    pub(super) percent: Option<u8>,
    pub(super) flood: Flood,
    pub(super) stopping: Option<(Phase, Instant, After)>,
    pub(super) output_ended: bool,
    pub(super) diagnostics_dropped: usize,
    /// When `*LSP*` last said how many were dropped: at most once a second.
    pub(super) diagnostics_said: Instant,
    pub(super) look_at: Instant,
    /// The encoding positions are in, once `initialize` said.
    pub(super) encoding: crate::position::Encoding,
    /// How the server wants documents synced, once `initialize` said.
    pub(super) sync: protocol::Sync,
}

impl Run {
    /// Sends request `method`, which must be answered within `wait`.
    pub(super) fn request(
        &mut self,
        asked: Asked,
        method: &str,
        params: Value,
        wait: Duration,
    ) -> Result<(), String> {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, (asked, Instant::now() + wait));
        self.send(&protocol::request(id, method, params))
    }

    /// How many bytes wait for the writer, none once it is gone.
    pub(super) fn queued(&self) -> usize {
        self.writer.as_ref().map_or(0, Writer::queued)
    }

    pub(super) fn send(&self, body: &[u8]) -> Result<(), String> {
        match &self.writer {
            Some(writer) => writer.send(body).map_err(|_| queue_full()),
            None => Ok(()),
        }
    }

    /// A request whose wait has ended, the earliest.
    pub(super) fn overdue(&self, now: Instant) -> Option<(Asked, Instant)> {
        self.pending
            .values()
            .filter(|(_, deadline)| *deadline <= now)
            .min_by_key(|(_, deadline)| *deadline)
            .copied()
    }

    /// The asks whose wait has ended, taken out of what is in flight.
    pub(super) fn overdue_asks(&mut self, now: Instant) -> Vec<(i64, super::super::Ask)> {
        let ids: Vec<i64> = self
            .pending
            .iter()
            .filter(|(_, (asked, deadline))| matches!(asked, Asked::Place(_)) && *deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| match self.pending.remove(&id) {
                Some((Asked::Place(ask), _)) => Some((id, ask)),
                _ => None,
            })
            .collect()
    }

    /// The first moment something is due.
    pub(super) fn next_deadline(&self) -> Instant {
        let requests = self.pending.values().map(|(_, deadline)| *deadline);
        let stop = self.stopping.iter().map(|(_, deadline, _)| *deadline);
        requests.chain(stop).fold(self.look_at, Instant::min)
    }

    /// Begins a stop the person asked for, then `after`: `shutdown` if the server was
    /// initialized, `exit` at once otherwise. A second ask only changes what follows.
    pub(super) fn stop(&mut self, after: After, now: Instant) {
        if let Some(stopping) = &mut self.stopping {
            stopping.2 = after;
            return;
        }
        if self.ready && self.writer.is_some() {
            self.stopping = Some((Phase::Shutdown, now + SHUTDOWN_WAIT, after));
            let _ = self.request(Asked::Shutdown, "shutdown", Value::Null, SHUTDOWN_WAIT);
        } else {
            self.stopping = Some((Phase::Shutdown, now, after));
            self.exit(now);
        }
    }

    /// Sends `exit` and closes the server's input, then waits for it to end.
    pub(super) fn exit(&mut self, now: Instant) {
        let Some((phase, deadline, _)) = &mut self.stopping else {
            return;
        };
        if *phase != Phase::Shutdown {
            return;
        }
        *phase = Phase::Exit;
        *deadline = now + EXIT_WAIT;
        self.pending
            .retain(|_, (asked, _)| *asked != Asked::Shutdown);
        let _ = self.send(&protocol::notification("exit", Value::Null));
        drop(self.writer.take());
        self.look_at = now;
    }

    /// Counts a message of `bytes`; `Err` when it floods.
    pub(super) fn flooded(&mut self, now: Instant, bytes: u64) -> Result<(), String> {
        if self.flood.count(now, bytes) {
            return Err(format!(
                "sent more than {FLOOD_MESSAGES_MAX} messages or {} MiB within {} s",
                FLOOD_BYTES_MAX >> 20,
                FLOOD_WINDOW.as_secs()
            ));
        }
        Ok(())
    }

    pub(super) fn progress(&mut self, token: Token, progress: Progress) {
        match progress {
            Progress::Working(percent) => {
                if self.tokens.contains_key(&token) || self.tokens.len() < PROGRESS_TOKENS_MAX {
                    self.tokens.insert(token, percent);
                    self.percent = percent.or(self.percent);
                }
            }
            Progress::Ended => {
                self.tokens.remove(&token);
                if self.tokens.is_empty() {
                    self.percent = None;
                }
            }
        }
    }

    pub(super) fn state(&self) -> State {
        if !self.ready {
            State::Starting
        } else if self.tokens.is_empty() {
            State::Ready
        } else {
            State::Working(self.percent)
        }
    }
}

fn queue_full() -> String {
    format!(
        "did not read what oso sent: more than {} MiB waited",
        WRITER_QUEUE_BYTES_MAX >> 20
    )
}
