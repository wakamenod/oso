//! The CPU time the calling thread has run, which the syntax worker's time limits are measured
//! in, so that a thread the system holds back on a loaded machine is not judged too slow
//! (ADR 0172). It asks the kernel, and reads no file.
//!
//! Must not depend on: `clipboard`, `control`, `file_meta`, `host`, `notify`, `privacy`,
//! `process`, `random`, `text` or `window`.

use std::time::Duration;

/// The CPU time the calling thread has run since it started, in user and kernel mode, if the
/// system says. Only the difference between two readings on one thread means anything. A read
/// costs a system call or less: about 0.1 µs on macOS.
///
/// On Windows it counts in the scheduler's ticks, about 15.6 ms each; on macOS and Linux it is
/// finer than a microsecond.
pub fn now() -> Option<Duration> {
    backend_now()
}

#[cfg(unix)]
fn backend_now() -> Option<Duration> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a valid `timespec` to write, which `clock_gettime` fills and keeps no
    // pointer to; the clock is the calling thread's, which every supported Unix has.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut time) };
    if result != 0 {
        return None;
    }
    let seconds = u64::try_from(time.tv_sec).ok()?;
    let nanos = u32::try_from(time.tv_nsec)
        .ok()
        .filter(|&n| n < 1_000_000_000)?;
    Some(Duration::new(seconds, nanos))
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::time::Duration;

    /// `FILETIME`: 100 ns intervals, in two halves.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn duration(self) -> Duration {
            let intervals = u64::from(self.high) << 32 | u64::from(self.low);
            Duration::from_nanos(intervals).saturating_mul(100)
        }
    }

    // kernel32, which every Windows process has loaded and std links. The declarations match
    // the Win32 prototypes: `HANDLE GetCurrentThread(void)` and `BOOL GetThreadTimes(HANDLE,
    // LPFILETIME, LPFILETIME, LPFILETIME, LPFILETIME)`, `BOOL` being a 32-bit int.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn GetThreadTimes(
            thread: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }

    pub(super) fn now() -> Option<Duration> {
        let mut times = [FileTime::default(); 4];
        let [creation, exit, kernel, user] = &mut times;
        // SAFETY: `GetCurrentThread` takes nothing and returns a pseudo-handle to the calling
        // thread, which needs no closing; `GetThreadTimes` writes one `FILETIME` through each
        // of the four pointers, all to distinct, valid locals, and keeps none of them.
        let result = unsafe { GetThreadTimes(GetCurrentThread(), creation, exit, kernel, user) };
        (result != 0).then(|| kernel.duration().saturating_add(user.duration()))
    }
}

#[cfg(windows)]
use win32::now as backend_now;

#[cfg(not(any(unix, windows)))]
fn backend_now() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::now;

    #[cfg(any(unix, windows))]
    #[test]
    fn grows_while_the_thread_works_and_not_while_it_sleeps() {
        let before = now().unwrap();
        // Busy for 50 ms of wall time, which on any machine is some CPU time: past Windows'
        // 15.6 ms tick unless the thread was held back nearly all of it.
        let started = Instant::now();
        let mut spun = 0_u64;
        while started.elapsed() < Duration::from_millis(50) {
            spun = std::hint::black_box(spun.wrapping_add(1));
        }
        let worked = now().unwrap();
        assert!(worked > before, "{before:?} then {worked:?}");
        std::thread::sleep(Duration::from_millis(50));
        let slept = now().unwrap();
        // The sleep's own system calls cost far less than its 50 ms.
        assert!(
            slept - worked < Duration::from_millis(20),
            "{worked:?} then {slept:?}"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn is_the_calling_threads_own() {
        // This thread runs 60 ms of CPU time first; a new one has run for a moment only.
        let before = now().unwrap();
        let mut spun = 0_u64;
        while now().unwrap() - before < Duration::from_millis(60) {
            spun = std::hint::black_box(spun.wrapping_add(1));
        }
        let theirs = std::thread::spawn(|| now().unwrap()).join().unwrap();
        assert!(theirs < Duration::from_millis(30), "{theirs:?}");
    }
}
