//! Asking this process's own parent to end (ADR 0151, step 2): `oso mcp` stops the Claude
//! Code session that started it once that session moved into oso, and never any other process.
//!
//! Must not depend on: `control`, `text` or the processes this module's [`super`] starts; it
//! reads this process's parent and signals it alone.
//!
//! - Only the parent the caller names, and only while it is this process's parent: once a
//!   parent ends, its child is given to another process and the parent's id can be reused, so
//!   the id is checked right before the signal.
//! - Linux holds the parent with a pidfd taken between two checks, so that the signal reaches
//!   the process checked and no later one. macOS has no such handle: the id is checked and then
//!   signalled at once, and a parent that ends keeps its id until its own parent reaps it.
//! - `SIGTERM` only, which Claude Code takes as a request to end, never `SIGKILL`; and never
//!   the first process, which an orphan's parent becomes.

use std::io;

/// Sends `SIGTERM` to `parent`, if it is this process's parent now.
pub(super) fn terminate(parent: u32) -> io::Result<()> {
    let mine = || std::os::unix::process::parent_id();
    if parent <= 1 || mine() != parent {
        return Err(not_the_parent());
    }
    let id = libc::pid_t::try_from(parent).map_err(|_| not_the_parent())?;
    #[cfg(target_os = "linux")]
    if let Some(handle) = linux::open(id)? {
        // The parent may have ended between the first check and the handle: the handle then
        // names a later process, and this one has another parent.
        if mine() != parent {
            return Err(not_the_parent());
        }
        return linux::terminate(&handle);
    }
    // SAFETY: `kill` takes and returns plain numbers. `id` was this process's parent a moment
    // ago, and a process that ends keeps its id until its own parent reaps it, so it names no
    // other process unless that parent ended and was reaped in between.
    if unsafe { libc::kill(id, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// What the caller is told when `parent` is not this process's parent.
fn not_the_parent() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "that process is not this process's parent",
    )
}

/// Linux's handle on a process, which no later process with its id can stand for.
#[cfg(target_os = "linux")]
mod linux {
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

    /// A pidfd of `id`, or `None` on a kernel older than 5.3, which has none.
    pub(super) fn open(id: libc::pid_t) -> io::Result<Option<OwnedFd>> {
        // SAFETY: `pidfd_open` takes a process id and flags, and returns a new descriptor or
        // -1; no memory is passed.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, id, 0) };
        if fd < 0 {
            let e = io::Error::last_os_error();
            return match e.raw_os_error() {
                Some(libc::ENOSYS) => Ok(None),
                _ => Err(e),
            };
        }
        let fd = RawFd::try_from(fd).map_err(|_| io::Error::other("a pidfd out of range"))?;
        // SAFETY: `fd` is the descriptor `pidfd_open` just made, owned by nothing else.
        Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Sends `SIGTERM` to the process `handle` holds.
    pub(super) fn terminate(handle: &OwnedFd) -> io::Result<()> {
        // SAFETY: `pidfd_send_signal` takes a pidfd this process owns, a signal, no `siginfo`
        // (a null pointer, which it allows) and no flags.
        let sent = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                handle.as_raw_fd(),
                libc::SIGTERM,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if sent == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::terminate;

    #[test]
    fn a_process_that_is_not_the_parent_is_never_signalled() {
        // Refused before any signal: neither the first process nor an id that is not this
        // process's parent. No test here signals its real parent, which is the test runner.
        let parent = std::os::unix::process::parent_id();
        for other in [0, 1, parent.wrapping_add(1), u32::MAX] {
            let refused = terminate(other).unwrap_err();
            assert_eq!(refused.kind(), std::io::ErrorKind::NotFound, "{other}");
        }
    }
}
