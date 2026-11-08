//! The process at the other end of a control socket, and the process that started it, as the
//! system reports them, so that the agent channel can name them to the person (ADR 0085).
//!
//! Must not depend on: the protocol spoken over the socket, or anything that decides from what
//! it finds: a process ID can be reused, so this names and proves nothing.
//!
//! - macOS: `LOCAL_PEERPID`, then `proc_pidinfo` for the name and the parent.
//! - Linux: `SO_PEERCRED`, then `/proc/<pid>/comm` and `/proc/<pid>/stat`.
//! - Elsewhere: nothing.

use std::os::unix::net::UnixStream;

use super::super::{Peer, Process, process_name};

/// Who is at the other end of `stream`. What the system does not say stays `None`.
pub fn of(stream: &UnixStream) -> Peer {
    let Some(pid) = peer_pid(stream) else {
        return Peer::default();
    };
    let (name, parent) = describe(pid);
    let parent = parent.filter(|&ppid| ppid != 0).map(|ppid| Process {
        pid: ppid,
        name: describe(ppid).0,
    });
    Peer {
        process: Some(Process { pid, name }),
        parent,
    }
}

/// The process ID of the peer.
#[cfg(target_os = "macos")]
fn peer_pid(stream: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let fd = stream.as_raw_fd();
    let mut pid: libc::pid_t = 0;
    let size = std::mem::size_of::<libc::pid_t>();
    let mut len = libc::socklen_t::try_from(size).ok()?;
    // SAFETY: `fd` belongs to `stream`, which is borrowed for the call. `pid` and `len` are
    // initialized locals that outlive the call, and `len` holds the size of `pid`, so the
    // kernel writes within it.
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &raw mut len,
        )
    };
    if ret != 0 || usize::try_from(len).ok() != Some(size) {
        return None;
    }
    u32::try_from(pid).ok().filter(|&pid| pid != 0)
}

/// The process ID of the peer.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_pid(stream: &UnixStream) -> Option<u32> {
    let cred = super::peer_cred(stream).ok()?;
    u32::try_from(cred.pid).ok().filter(|&pid| pid != 0)
}

/// The process ID of the peer: not known here.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
fn peer_pid(_stream: &UnixStream) -> Option<u32> {
    None
}

/// The name of the process `pid`, and its parent's process ID.
#[cfg(target_os = "macos")]
fn describe(pid: u32) -> (Option<String>, Option<u32>) {
    let Ok(pid) = libc::c_int::try_from(pid) else {
        return (None, None);
    };
    // SAFETY: `proc_bsdinfo` is plain data, for which all zeroes is a valid value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let Ok(size) = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()) else {
        return (None, None);
    };
    // SAFETY: `info` is an initialized local that outlives the call, and `size` is its size, so
    // the kernel writes within it.
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if written != size {
        return (None, None);
    }
    // The long name, and the short one when there is none.
    let name = c_text(&info.pbi_name).or_else(|| c_text(&info.pbi_comm));
    (name, Some(info.pbi_ppid))
}

/// The text of a fixed C array up to its first NUL, as a process name.
#[cfg(target_os = "macos")]
fn c_text(chars: &[libc::c_char]) -> Option<String> {
    let bytes: Vec<u8> = chars
        .iter()
        .map(|&c| c.cast_unsigned())
        .take_while(|&b| b != 0)
        .collect();
    process_name(&String::from_utf8_lossy(&bytes))
}

/// The name of the process `pid`, and its parent's process ID.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn describe(pid: u32) -> (Option<String>, Option<u32>) {
    // A few bytes each, so the reads are bounded; a process that is gone gives nothing.
    let name = std::fs::read(format!("/proc/{pid}/comm"))
        .ok()
        .and_then(|bytes| process_name(String::from_utf8_lossy(&bytes).trim_end()));
    let parent = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| parent_in_stat(&stat));
    (name, parent)
}

/// The parent's process ID in a `/proc/<pid>/stat` line: the field after the state, which
/// follows the name in parentheses. The name may hold spaces and parentheses itself, so this
/// starts after the last `)`.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn parent_in_stat(stat: &str) -> Option<u32> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Nothing is known here.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
fn describe(_pid: u32) -> (Option<String>, Option<u32>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::{of, parent_in_stat};
    use std::os::unix::net::UnixStream;

    #[test]
    fn reads_the_parent_after_the_name() {
        assert_eq!(parent_in_stat("812 (claude) S 1 812 812 0"), Some(1));
        assert_eq!(parent_in_stat("9 (a) b) (c) R 77 9"), Some(77));
        assert_eq!(parent_in_stat("9 (a"), None);
        assert_eq!(parent_in_stat("9 (a) S"), None);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn names_this_process_and_its_parent() {
        let (a, _b) = UnixStream::pair().unwrap();
        let peer = of(&a);
        let process = peer.process.unwrap();
        assert_eq!(process.pid, std::process::id());
        assert!(process.name.is_some_and(|name| !name.is_empty()));
        assert!(peer.parent.is_some_and(|parent| parent.pid != process.pid));
    }
}
