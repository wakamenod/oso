//! A native resume of a session whose TUI's `claude` may still run waits, on its start
//! thread, until that process is gone (ADR 0165, part 4d-2): never two writers of one
//! conversation. It is the start's first step, before the directory's and the registry's looks.
//!
//! Must not depend on: `app` or the UI thread; it runs on the session's start thread.
//!
//! - A TUI's terminal says it ended once its pty did, and gives up waiting for its child 2 s
//!   later (`oso-term`), and a kill of its buffer stops the session before its hang-up even
//!   goes, so a `claude` that outlives its terminal, as one that ignores `SIGHUP` does, may
//!   still run. The session keeps its TUI's process id and start time, read on the TUI's start
//!   thread just after the spawn (`Hosted::tui_process`), until its end is seen reaped.
//! - The process counts as running while one with the id started when it did: a process id is
//!   reused only once its process ended and was reaped, and a start time read twice from the
//!   system is read the same, within a second, as a Linux start time counts from a boot time
//!   in whole seconds. On macOS a change of the wall clock moves neither read. With no start
//!   time known, any process with the id counts as it; so does one whose start the system
//!   does not say.
//! - Past [`WAIT`], the resume is refused, saying why: the session stays stopped, and its next
//!   native resume waits again.

use std::io;
use std::time::{Duration, Instant, SystemTime};

use super::super::hosted::TuiProcess;
use super::{Order, Report, Runs};

/// How long the TUI's `claude` is waited for.
pub const WAIT: Duration = Duration::from_secs(10);

/// How often it is looked at.
const EVERY: Duration = Duration::from_millis(50);

/// How far apart two reads of one process's start time may be.
const SAME_START: Duration = Duration::from_secs(1);

/// What holds `order` back: a native resume whose session's TUI `claude` is still running past
/// [`WAIT`].
pub(super) fn held_for_tui(order: &Order) -> Option<Report> {
    let Runs::Native(native) = &order.runs else {
        return None;
    };
    let process = native.tui_to_end.as_ref()?;
    wait(process, WAIT, oso_platform::process::start_time)
        .err()
        .map(|why| Report::Refused {
            token: order.token,
            why,
        })
}

/// Waits until `process` is gone, as `started` says when a process id's process started, at
/// most `wait`.
///
/// # Errors
///
/// Why the resume does not go: the process still runs, or cannot be told from another.
pub fn wait(
    process: &TuiProcess,
    wait: Duration,
    started: impl Fn(u32) -> io::Result<Option<SystemTime>>,
) -> Result<(), String> {
    let until = Instant::now() + wait;
    loop {
        if !runs(process, &started) {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(format!(
                "The session did not resume natively: its TUI's claude, process {}, still \
                 runs, and one process writes a conversation at a time",
                process.pid
            ));
        }
        std::thread::sleep(EVERY);
    }
}

/// Whether `process` still runs, as far as `started` tells.
fn runs(process: &TuiProcess, started: &impl Fn(u32) -> io::Result<Option<SystemTime>>) -> bool {
    match (started(process.pid), process.started) {
        (Ok(None), _) => false,
        (Ok(Some(now)), Some(then)) => same_start(now, then),
        (Ok(Some(_)) | Err(_), _) => true,
    }
}

/// Whether two reads of a start time are of one start.
fn same_start(a: SystemTime, b: SystemTime) -> bool {
    let apart = a.duration_since(b).unwrap_or_else(|e| e.duration());
    apart < SAME_START
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io;
    use std::time::{Duration, Instant, SystemTime};

    use super::{TuiProcess, wait};

    fn started_at(at: SystemTime) -> TuiProcess {
        TuiProcess {
            pid: 42,
            started: Some(at),
        }
    }

    #[test]
    fn a_tui_that_is_gone_or_whose_id_is_anothers_lets_the_resume_go() {
        let then = SystemTime::now() - Duration::from_secs(600);
        let process = started_at(then);
        assert_eq!(wait(&process, Duration::ZERO, |_| Ok(None)), Ok(()));
        // Another process took the id since: it started later.
        let later = then + Duration::from_secs(5);
        assert_eq!(wait(&process, Duration::ZERO, |_| Ok(Some(later))), Ok(()));
    }

    #[test]
    fn a_tui_that_still_runs_or_cannot_be_told_refuses_the_resume() {
        let then = SystemTime::now() - Duration::from_secs(600);
        let process = started_at(then);
        let still = wait(&process, Duration::ZERO, |_| Ok(Some(then)));
        assert!(still.unwrap_err().contains("process 42, still runs"));
        // A second off, as Linux may read it: the same start.
        let off = then + Duration::from_millis(900);
        assert!(wait(&process, Duration::ZERO, |_| Ok(Some(off))).is_err());
        let unknown = |_| Err(io::Error::other("not ours to read"));
        assert!(wait(&process, Duration::ZERO, unknown).is_err());
        // With no start known, any process with the id is the TUI's.
        let unread = TuiProcess {
            pid: 42,
            started: None,
        };
        assert!(wait(&unread, Duration::ZERO, |_| Ok(Some(then))).is_err());
        assert_eq!(wait(&unread, Duration::ZERO, |_| Ok(None)), Ok(()));
    }

    /// A start time in the future, as the wall clock set back since makes it, is still the
    /// TUI's: the start times read are compared with each other, not with the clock.
    #[test]
    fn a_start_after_the_clock_now_is_still_the_tuis() {
        let future = SystemTime::now() + Duration::from_secs(3600);
        let process = started_at(future);
        assert!(wait(&process, Duration::ZERO, |_| Ok(Some(future))).is_err());
    }

    #[test]
    fn a_tui_that_ends_while_waited_for_lets_the_resume_go() {
        let then = SystemTime::now() - Duration::from_secs(600);
        let process = started_at(then);
        let looks = Cell::new(0);
        let ends = |_| {
            looks.set(looks.get() + 1);
            Ok((looks.get() < 3).then_some(then))
        };
        assert_eq!(wait(&process, Duration::from_secs(5), ends), Ok(()));
        assert_eq!(looks.get(), 3);
    }

    /// A TUI that never ends is given up at the bound, not later.
    #[test]
    fn a_tui_that_never_ends_is_given_up_at_the_bound() {
        let then = SystemTime::now() - Duration::from_secs(600);
        let process = started_at(then);
        let looks = Cell::new(0);
        let runs = |_| {
            looks.set(looks.get() + 1);
            Ok(Some(then))
        };
        let began = Instant::now();
        assert!(wait(&process, Duration::from_millis(200), runs).is_err());
        let took = began.elapsed();
        assert!(took >= Duration::from_millis(200), "{took:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert!((4..=6).contains(&looks.get()), "{}", looks.get());
    }
}
