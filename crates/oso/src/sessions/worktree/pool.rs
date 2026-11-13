//! The threads that run worktree jobs, `oso-sessions-git` (ADR 0121, "One sessions worker"): at
//! most [`MAX_THREADS`] at once, each git or `gh` call on them bounded to 10 s, so a slow
//! repository delays other jobs, never the UI or the sessions worker's rules.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the UI thread.
//!
//! Nothing runs while nothing is asked: a thread starts when a job waits and none is free,
//! and ends after [`IDLE`] with no job. A waiting thread woken for a job counts as taken, so a
//! second job asked for at once starts a second thread. A job the person waits on, as a
//! prompt's branches, goes ahead of the others.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

/// The most jobs that run at once.
pub const MAX_THREADS: usize = 2;
/// How long a thread with no job waits for one before it ends.
pub const IDLE: Duration = Duration::from_secs(30);

type Work = Box<dyn FnOnce() + Send + 'static>;

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Work>,
    /// Threads running.
    threads: usize,
    /// Threads waiting for a job.
    waiting: usize,
    /// Wakes sent to waiting threads that none has taken yet, each for one job.
    wakes: usize,
}

/// The pool. Cloning it shares it.
#[derive(Clone, Default)]
pub struct Pool {
    shared: Arc<(Mutex<Queue>, Condvar)>,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pool")
    }
}

impl Pool {
    /// Runs `work` on one of the threads, after the jobs asked for before it. `false` if no
    /// thread could start and none runs: the job is dropped.
    pub fn run(&self, work: impl FnOnce() + Send + 'static) -> bool {
        self.push(Box::new(work), false)
    }

    /// Runs `work` on one of the threads before the jobs that wait, as for a prompt the person
    /// waits on. `false` as [`Pool::run`].
    pub fn run_first(&self, work: impl FnOnce() + Send + 'static) -> bool {
        self.push(Box::new(work), true)
    }

    fn push(&self, work: Work, first: bool) -> bool {
        let (lock, wake) = &*self.shared;
        let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if first {
            queue.jobs.push_front(work);
        } else {
            queue.jobs.push_back(work);
        }
        if queue.waiting > queue.wakes {
            queue.wakes += 1;
            wake.notify_one();
            return true;
        }
        if queue.threads >= MAX_THREADS {
            return true;
        }
        queue.threads += 1;
        let shared = Arc::clone(&self.shared);
        let started = thread::Builder::new()
            .name("oso-sessions-git".into())
            .spawn(move || serve(&shared));
        if let Err(e) = started {
            log::warn!("a worktree job's thread cannot start: {e}");
            queue.threads -= 1;
            if queue.threads == 0 {
                if first {
                    queue.jobs.pop_front();
                } else {
                    queue.jobs.pop_back();
                }
                return false;
            }
        }
        true
    }
}

/// Takes jobs until none comes for [`IDLE`].
fn serve(shared: &(Mutex<Queue>, Condvar)) {
    let (lock, wake) = shared;
    let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        if let Some(work) = queue.jobs.pop_front() {
            drop(queue);
            // A job that panics ends only itself; the thread takes the next.
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
                log::warn!("a worktree job failed");
            }
            queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
            continue;
        }
        queue.waiting += 1;
        let (next, timeout) = wake
            .wait_timeout(queue, IDLE)
            .unwrap_or_else(PoisonError::into_inner);
        queue = next;
        queue.waiting -= 1;
        queue.wakes = queue.wakes.saturating_sub(1);
        if timeout.timed_out() && queue.jobs.is_empty() {
            queue.threads -= 1;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use super::{MAX_THREADS, Pool};

    #[test]
    fn jobs_run_and_at_most_two_at_once() {
        let pool = Pool::default();
        let (sent, got) = mpsc::channel();
        let gate = Arc::new(Barrier::new(MAX_THREADS));
        for n in 0..6 {
            let sent = sent.clone();
            let gate = Arc::clone(&gate);
            assert!(pool.run(move || {
                if n < MAX_THREADS {
                    // The first two meet here, so both run at once.
                    gate.wait();
                }
                sent.send(n).unwrap();
            }));
        }
        let mut done: Vec<usize> = (0..6)
            .map(|_| got.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect();
        done.sort_unstable();
        assert_eq!(done, [0, 1, 2, 3, 4, 5]);
        let threads = pool.shared.0.lock().unwrap().threads;
        assert!(threads <= MAX_THREADS);
    }

    #[test]
    fn a_burst_after_an_idle_thread_starts_a_second_one() {
        let pool = Pool::default();
        let (sent, got) = mpsc::channel();
        let first = sent.clone();
        assert!(pool.run(move || first.send(()).unwrap()));
        got.recv_timeout(Duration::from_secs(10)).unwrap();
        // The thread is waiting for a job now.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while pool.shared.0.lock().unwrap().waiting == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the thread never waited"
            );
            std::thread::yield_now();
        }
        let gate = Arc::new(Barrier::new(MAX_THREADS));
        for _ in 0..MAX_THREADS {
            let (sent, gate) = (sent.clone(), Arc::clone(&gate));
            // Both meet here, so both must run at once.
            assert!(pool.run(move || {
                gate.wait();
                sent.send(()).unwrap();
            }));
        }
        for _ in 0..MAX_THREADS {
            assert!(got.recv_timeout(Duration::from_secs(10)).is_ok());
        }
    }

    #[test]
    fn a_job_asked_first_runs_before_those_waiting() {
        let pool = Pool::default();
        let (sent, got) = mpsc::channel();
        let gate = Arc::new(Barrier::new(MAX_THREADS + 1));
        let (held, holding) = mpsc::channel();
        // Every thread is held, so what comes next waits.
        for _ in 0..MAX_THREADS {
            let (gate, held) = (Arc::clone(&gate), held.clone());
            assert!(pool.run(move || {
                held.send(()).unwrap();
                gate.wait();
            }));
        }
        for _ in 0..MAX_THREADS {
            holding.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        for (n, first) in [(1, false), (2, false), (0, true)] {
            let sent = sent.clone();
            let work = move || sent.send(n).unwrap();
            assert!(if first {
                pool.run_first(work)
            } else {
                pool.run(work)
            });
        }
        // The queue's front is the job asked for first.
        let front = pool.shared.0.lock().unwrap().jobs.pop_front().unwrap();
        front();
        assert_eq!(got.recv_timeout(Duration::from_secs(10)), Ok(0));
        gate.wait();
        let mut rest: Vec<i32> = (0..2)
            .map(|_| got.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect();
        rest.sort_unstable();
        assert_eq!(rest, [1, 2]);
    }

    #[test]
    fn a_job_that_panics_leaves_the_pool_working() {
        let pool = Pool::default();
        let (sent, got) = mpsc::channel();
        pool.run(|| panic!("a job's own failure"));
        pool.run(move || sent.send(()).unwrap());
        assert!(got.recv_timeout(Duration::from_secs(10)).is_ok());
    }
}
