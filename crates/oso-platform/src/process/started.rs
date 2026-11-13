//! When a process started, by its process ID, so that a caller can tell the process a
//! record names from another that took its ID later (ADR 0121: the session registry's
//! liveness check).
//!
//! Must not depend on: `control` or `text`.
//!
//! - macOS: `proc_pidinfo` with `PROC_PIDTBSDINFO`, to the microsecond.
//! - Linux: `/proc/<pid>/stat`'s start time in clock ticks since boot, plus `/proc/stat`'s
//!   boot time, to the tick; the boot time is whole seconds, so it can be a second off.
//! - Elsewhere: nothing is known.

use std::time::SystemTime;

/// When the process `pid` started, or `None` if there is no such process or the system does
/// not say.
pub(super) fn of(pid: u32) -> Option<SystemTime> {
    imp::of(pid)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::time::{Duration, SystemTime};

    pub(super) fn of(pid: u32) -> Option<SystemTime> {
        let pid = libc::c_int::try_from(pid).ok().filter(|&pid| pid > 0)?;
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
        // SAFETY: `info` is a local of `size` bytes that outlives the call, so the kernel
        // writes within it; it returns how many bytes it wrote.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if written != size {
            return None;
        }
        // SAFETY: the kernel filled all `size` bytes of it, and `proc_bsdinfo` is plain data,
        // integers and arrays of `c_char`, for which any bytes are a valid value.
        let info = unsafe { info.assume_init() };
        let since = Duration::from_secs(info.pbi_start_tvsec)
            + Duration::from_micros(info.pbi_start_tvusec);
        SystemTime::UNIX_EPOCH.checked_add(since)
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use std::time::{Duration, SystemTime};

    pub(super) fn of(pid: u32) -> Option<SystemTime> {
        // A few hundred bytes each, so the reads are bounded; a process that is gone gives
        // nothing.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let ticks = super::start_ticks(&stat)?;
        let boot = std::fs::read_to_string("/proc/stat").ok()?;
        let boot = super::boot_seconds(&boot)?;
        // SAFETY: `sysconf` reads a configuration value and touches no memory of ours.
        let per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let per_second = u64::try_from(per_second).ok().filter(|&n| n > 0)?;
        let since = Duration::from_secs(boot)
            + Duration::from_secs(ticks / per_second)
            + Duration::from_nanos((ticks % per_second) * 1_000_000_000 / per_second);
        SystemTime::UNIX_EPOCH.checked_add(since)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
mod imp {
    use std::time::SystemTime;

    pub(super) fn of(_pid: u32) -> Option<SystemTime> {
        None
    }
}

/// The start time in a `/proc/<pid>/stat` line, in clock ticks since boot: its 22nd field.
/// The name, the 2nd, is in parentheses and may hold spaces and parentheses itself, so the
/// count starts after the last `)`, at the 3rd.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn start_ticks(stat: &str) -> Option<u64> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(22 - 3)?.parse().ok()
}

/// The boot time in `/proc/stat`, in seconds since the epoch: its `btime` line.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn boot_seconds(stat: &str) -> Option<u64> {
    stat.lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::{boot_seconds, start_ticks};

    #[test]
    fn reads_the_start_after_the_name_whatever_the_name_holds() {
        let stat = "42 (a) b (c) S 1 42 42 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 98765 \
                    1000 10 18446744073709551615";
        assert_eq!(start_ticks(stat), Some(98765));
        assert_eq!(start_ticks("42 (gone"), None);
        assert_eq!(start_ticks("42 (short) S 1 2"), None);
    }

    #[test]
    fn reads_the_boot_time_line() {
        assert_eq!(
            boot_seconds("cpu 1 2 3\nintr 5\nbtime 1700000000\nprocesses 9\n"),
            Some(1_700_000_000)
        );
        assert_eq!(boot_seconds("cpu 1 2 3\n"), None);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn this_process_and_its_parent_started_in_the_past_and_no_process_did_not() {
        use super::of;
        use std::time::{Duration, SystemTime};

        let now = SystemTime::now();
        let ours = of(std::process::id()).unwrap();
        assert!(
            ours <= now + Duration::from_secs(2),
            "{ours:?} after {now:?}"
        );
        let parent = of(std::os::unix::process::parent_id()).unwrap();
        // Linux counts from a boot time in whole seconds.
        assert!(
            parent <= ours + Duration::from_secs(1),
            "the parent started after us"
        );
        assert_eq!(of(0), None);
        assert_eq!(of(u32::MAX), None);
    }
}
