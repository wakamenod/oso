//! The syntax worker's threads (ADR 0111): two, each on an 8 MiB stack, sharing one schedule
//! behind a lock that is held only to take or give back a document, never during a slice.
//!
//! Must not depend on: the editor; it sees orders, documents and reports.
//!
//! - Nothing under the lock calls tree-sitter: documents are made, edited and freed by a thread
//!   with nothing locked, so the UI never waits on that, here or for the lock.
//! - Two threads, and at most one on a buffer no window shows or a document no buffer owns
//!   (`oso_syntax::Scheduler`): a slice that runs long, as tree-sitter checks its deadline only
//!   so often, holds one thread, and shown buffers keep the other.
//! - The stack is set, not left to the platform: Markdown's inline grammar recurses on nested
//!   links, and an inline node at its cap needs about 512 KiB (ADR 0111, as built).
//! - A slice that panics loses its buffer or document only; its thread compiles its queries
//!   afresh and goes on. Nothing logged names a buffer or quotes its text.
//! - Dropping the pool tells the threads to stop; they end at their next wait.
//! - It is generic over the schedule's key, so that documents no buffer owns (ADR 0122) share
//!   the threads with buffers under keys of their own.

use std::hash::Hash;
use std::io;
use std::ops::Range;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;

use oso_core::buffer::Verdict;
use oso_core::rope::Rope;
use oso_syntax::{Language, Queries, Refused, Report, Scheduler};

use super::Orders;

/// How many threads highlight.
const THREADS: usize = 2;

/// Each thread's stack: 16 times what the inline cap needs (ADR 0111, as built).
const STACK: usize = 8 << 20;

/// The threads, through what they share.
pub(super) struct Pool<Key> {
    shared: Arc<Shared<Key>>,
}

struct Shared<Key> {
    state: Mutex<State<Key>>,
    /// Wakes a thread when there may be work, or when the pool stops.
    ready: Condvar,
}

struct State<Key> {
    scheduler: Scheduler<Key>,
    stopped: bool,
}

impl<Key: Copy + Eq + Hash + Send + 'static> Pool<Key> {
    /// Starts the threads, each of which hands its reports to `report`: the app's waker, or a
    /// test's channel.
    pub(super) fn start(report: impl Fn(Report<Key>) + Clone + Send + 'static) -> io::Result<Self> {
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
    pub(super) fn send(&self, orders: Orders<Key>) {
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

    /// Opens a document no buffer owns under `key`, as [`Scheduler::open`] says: refused, and
    /// nothing kept, for a text that may hold secrets. The lock is held for that alone.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "ADR 0122 step 9 opens review sides")
    )]
    pub(super) fn open(
        &self,
        key: Key,
        language: Language,
        verdict: Verdict,
        text: Rope,
        shown: Vec<Range<usize>>,
    ) -> Result<(), Refused> {
        // O(1): so that a refused text is freed here, after the lock, not under it.
        let kept = text.clone();
        let opened = lock(&self.shared.state)
            .scheduler
            .open(key, language, verdict, text, shown);
        // There is work, or a replaced document to free.
        self.shared.ready.notify_all();
        drop(kept);
        opened
    }

    /// What windows show of an open document now.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "ADR 0122 step 9 opens review sides")
    )]
    pub(super) fn show(&self, key: Key, shown: Vec<Range<usize>>) {
        lock(&self.shared.state).scheduler.show(key, shown);
        self.shared.ready.notify_all();
    }

    /// Closes a document at once: nothing more runs on it, and a thread frees it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "ADR 0122 step 9 opens review sides")
    )]
    pub(super) fn close(&self, key: Key) {
        lock(&self.shared.state).scheduler.close(key);
        self.shared.ready.notify_all();
    }
}

impl<Key> Drop for Pool<Key> {
    fn drop(&mut self) {
        lock(&self.shared.state).stopped = true;
        self.shared.ready.notify_all();
    }
}

/// A thread's loop: take a document and what is to free, free and run a slice with nothing
/// locked, give the document back, report.
fn work<Key: Copy + Eq + Hash>(shared: &Shared<Key>, send: &impl Fn(Report<Key>)) {
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
                    log::warn!("a syntax slice panicked; that text stays plain");
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
fn lock<Key>(state: &Mutex<State<Key>>) -> MutexGuard<'_, State<Key>> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}
