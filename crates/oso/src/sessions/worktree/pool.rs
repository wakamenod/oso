//! The threads that run worktree jobs, `oso-sessions-git` (ADR 0121, "One sessions worker"): at
//! most [`MAX_THREADS`] at once, each git or `gh` call on them bounded to 10 s, so a slow
//! repository delays other jobs, never the UI or the sessions worker's rules.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the UI thread.
//!
//! Nothing runs while nothing is asked: a thread starts when a job waits and none is free,
//! and ends after [`IDLE`] with no job.

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
        let (lock, wake) = &*self.shared;
        let mut queue = lock.lock().unwrap_or_else(PoisonError::into_inner);
        queue.jobs.push_back(Box::new(work));
        if queue.waiting > 0 {
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
                queue.jobs.pop_back();
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
    fn a_job_that_panics_leaves_the_pool_working() {
        let pool = Pool::default();
        let (sent, got) = mpsc::channel();
        pool.run(|| panic!("a job's own failure"));
        pool.run(move || sent.send(()).unwrap());
        assert!(got.recv_timeout(Duration::from_secs(10)).is_ok());
    }
}
