//! The syntax worker's threads (ADR 0111): two, each on an 8 MiB stack, sharing one schedule
//! behind a lock that is held only to take or give back a document, never during a slice.
//!
//! Must not depend on: the editor; it sees orders, documents and reports.
//!
//! - Nothing under the lock calls tree-sitter: documents are made, edited and freed by a thread
//!   with nothing locked, so the UI never waits on that, here or for the lock.
//! - Two threads, and at most one on a buffer no window shows (`oso_syntax::Scheduler`): a
//!   slice that runs long, as tree-sitter checks its deadline only so often, holds one thread,
//!   and shown buffers keep the other.
//! - The stack is set, not left to the platform: Markdown's inline grammar recurses on nested
//!   links, and an inline node at its cap needs about 512 KiB (ADR 0111, as built).
//! - A slice that panics loses its buffer only; its thread compiles its queries afresh and goes
//!   on. Nothing logged names a buffer or quotes its text.
//! - Dropping the pool tells the threads to stop; they end at their next wait.

use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use oso_core::buffer::BufferId;
use oso_syntax::{Queries, Report, Scheduler};

use super::Orders;

/// How many threads highlight.
const THREADS: usize = 2;

/// Each thread's stack: 16 times what the inline cap needs (ADR 0111, as built).
const STACK: usize = 8 << 20;

/// The threads, through what they share.
pub(super) struct Pool {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    /// Wakes a thread when there may be work, or when the pool stops.
    ready: Condvar,
}

struct State {
    scheduler: Scheduler<BufferId>,
    stopped: bool,
}

impl Pool {
    /// Starts the threads, each of which hands its reports to `report`: the app's waker, or a
    /// test's channel.
    pub(super) fn start(
        report: impl Fn(Report<BufferId>) + Clone + Send + 'static,
    ) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                scheduler: Scheduler::new(),
                stopped: false,
            }),
            ready: Condvar::new(),
        });
        let pool = Self {
            shared: Arc::clone(&shared),
        };
        for index in 0..THREADS {
            let shared = Arc::clone(&shared);
            let report = report.clone();
            // If one cannot start, dropping the pool stops those that did.
            thread::Builder::new()
                .name(format!("oso-syntax-{index}"))
                .stack_size(STACK)
                .spawn(move || work(&shared, &report))?;
        }
        Ok(pool)
    }

    /// Hands the worker what the UI decided: drops first, so that nothing waiting for a
    /// dropped buffer runs, then jobs. The lock is held for that alone.
    pub(super) fn send(&self, orders: Orders) {
        let mut state = lock(&self.shared.state);
        for buffer in orders.drops {
            state.scheduler.drop_buffer(buffer);
        }
        for job in orders.jobs {
            state.scheduler.submit(job);
        }
        drop(state);
        // There is work, or documents to free.
        self.shared.ready.notify_all();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        lock(&self.shared.state).stopped = true;
        self.shared.ready.notify_all();
    }
}

/// A thread's loop: take a document and what is to free, free and run a slice with nothing
/// locked, give the document back, report.
fn work(shared: &Shared, send: &impl Fn(Report<BufferId>)) {
    let mut queries = Queries::new();
    loop {
        let (doomed, checkout) = {
            let mut state = lock(&shared.state);
            loop {
                if state.stopped {
                    return;
                }
                let doomed = state.scheduler.take_doomed();
                let checkout = state.scheduler.take_next();
                if !doomed.is_empty() || checkout.is_some() {
                    break (doomed, checkout);
                }
                state = shared
                    .ready
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        // Freeing a tree costs as much as its nodes: never under the lock.
        drop(doomed);
        let Some(mut checkout) = checkout else {
            continue;
        };
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| checkout.run(&mut queries)));
        let (report, doomed) = {
            let mut state = lock(&shared.state);
            let report = match outcome {
                Ok(outcome) => state.scheduler.give_back(checkout, outcome),
                Err(_) => {
                    log::warn!("a syntax slice panicked; that buffer stays plain");
                    queries = Queries::new();
                    state.scheduler.lost(checkout)
                }
            };
            (report, state.scheduler.take_doomed())
        };
        drop(doomed);
        // Another thread may now take what this one held back, or the background lane.
        shared.ready.notify_all();
        if let Some(report) = report {
            send(report);
        }
    }
}

/// The lock, also after a panic elsewhere: the schedule stays usable. Nothing under it calls
/// tree-sitter, makes or frees a document.
fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}
