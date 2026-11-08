//! The process at the other end of a control socket: the user it runs as, which the
//! transport checks (ADR 0032), and for the agent channel's question (ADR 0085) its name,
//! whether it runs this program, and the process that started it, as the system reports
//! them. A process ID can be reused, so all but the user names and proves nothing.
//!
//! Must not depend on: the protocol spoken over the socket.
//!
//! - macOS: `getpeereid` and `LOCAL_PEERPID`, then `proc_pidinfo` for the name and the
//!   parent, and `proc_pidpath` for the program.
//! - Linux: `SO_PEERCRED`, then `/proc/<pid>/comm`, `/proc/<pid>/stat` and `/proc/<pid>/exe`.
//! - Other Unix systems: every peer is refused, and nothing is named.

use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::control::{Peer, PeerProcess};

/// The user that owns the process at the other end of `stream`.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn owner(stream: &UnixStream) -> io::Result<u32> {
    cred(stream).map(|cred| cred.uid)
}

/// The credentials of the process at the other end of `stream`.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn cred(stream: &UnixStream) -> io::Result<libc::ucred> {
    use std::os::fd::AsRawFd;
    let fd = stream.as_raw_fd();
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let size = std::mem::size_of::<libc::ucred>();
    let mut len = libc::socklen_t::try_from(size).expect("ucred fits in socklen_t");
    // SAFETY: `fd` belongs to `stream`, which is borrowed for the call.
    // `cred` and `len` are initialized locals that outlive the call, and
    // `len` holds the size of `cred`, so the kernel writes within it.
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &raw mut len,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    if usize::try_from(len).ok() != Some(size) {
        return Err(io::Error::other("SO_PEERCRED returned a short ucred"));
    }
    Ok(cred)
}

/// The user that owns the process at the other end of `stream`.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
pub fn owner(stream: &UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;
    let fd = stream.as_raw_fd();
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `fd` belongs to `stream`, which is borrowed for the call, and
    // `uid` and `gid` are initialized locals that outlive the call.
    let ret = unsafe { libc::getpeereid(fd, &raw mut uid, &raw mut gid) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

/// Other Unix systems: no peer check is implemented, so every peer is
/// refused.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
pub fn owner(_stream: &UnixStream) -> io::Result<u32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the control socket cannot check peers on this system",
    ))
}

/// Who is at the other end of `stream`. What the system does not say stays `None`, and a
/// program it cannot name is not this one. Reads a few small files on Linux, so call this off
/// the UI thread.
pub fn of(stream: &UnixStream) -> Peer {
    let Some(pid) = peer_pid(stream) else {
        return Peer::default();
    };
    let (name, parent) = describe(pid);
    let parent = parent.filter(|&ppid| ppid != 0).map(|ppid| PeerProcess {
        pid: ppid,
        name: describe(ppid).0,
    });
    Peer {
        process: Some(PeerProcess { pid, name }),
        parent,
        runs_this_program: program(pid).is_some_and(|path| is_this_program(&path)),
    }
}

/// Whether `path` is this process's own program. Both sides are resolved, so a link to it
/// counts; anything that cannot be resolved does not.
fn is_this_program(path: &Path) -> bool {
    let ours = std::env::current_exe().and_then(std::fs::canonicalize);
    let theirs = std::fs::canonicalize(path);
    matches!((ours, theirs), (Ok(ours), Ok(theirs)) if ours == theirs)
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
    let cred = cred(stream).ok()?;
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
    // SAFETY: `proc_bsdinfo` is plain data, integers and arrays of `c_char`, for which all
    // zeroes is a valid value.
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

/// The program the process `pid` runs.
#[cfg(target_os = "macos")]
fn program(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut buffer = vec![0u8; usize::try_from(libc::PROC_PIDPATHINFO_MAXSIZE).ok()?];
    let size = u32::try_from(buffer.len()).ok()?;
    // SAFETY: `buffer` is an initialized allocation of `size` bytes that outlives the call, so
    // the kernel writes within it. It returns the length written, or 0 or less on failure.
    let written = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), size) };
    let written = usize::try_from(written).ok().filter(|&n| n > 0)?;
    buffer.truncate(written.min(buffer.len()));
    Some(PathBuf::from(std::ffi::OsString::from_vec(buffer)))
}

/// The text of a fixed C array up to its first NUL, as a process name.
#[cfg(target_os = "macos")]
fn c_text(chars: &[libc::c_char]) -> Option<String> {
    let bytes: Vec<u8> = chars
        .iter()
        .map(|&c| c.cast_unsigned())
        .take_while(|&b| b != 0)
        .collect();
    PeerProcess::clean_name(&String::from_utf8_lossy(&bytes))
}

/// The name of the process `pid`, and its parent's process ID.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn describe(pid: u32) -> (Option<String>, Option<u32>) {
    // A few bytes each, so the reads are bounded; a process that is gone gives nothing.
    let name = std::fs::read(format!("/proc/{pid}/comm"))
        .ok()
        .and_then(|bytes| PeerProcess::clean_name(String::from_utf8_lossy(&bytes).trim_end()));
    let parent = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| parent_in_stat(&stat));
    (name, parent)
}

/// The program the process `pid` runs.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn program(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
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

/// Nothing is known here.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
fn program(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::{is_this_program, of, parent_in_stat};
    use std::os::unix::net::UnixStream;

    #[test]
    fn reads_the_parent_after_the_name() {
        assert_eq!(parent_in_stat("812 (claude) S 1 812 812 0"), Some(1));
        assert_eq!(parent_in_stat("9 (a) b) (c) R 77 9"), Some(77));
        assert_eq!(parent_in_stat("9 (a"), None);
        assert_eq!(parent_in_stat("9 (a) S"), None);
    }

    #[test]
    fn only_this_program_is_this_program() {
        assert!(is_this_program(&std::env::current_exe().unwrap()));
        assert!(!is_this_program(std::path::Path::new("/bin/sh")));
        assert!(!is_this_program(std::path::Path::new("/no/such/program")));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn names_this_process_its_program_and_its_parent() {
        let (a, _b) = UnixStream::pair().unwrap();
        let peer = of(&a);
        let process = peer.process.unwrap();
        assert_eq!(process.pid(), std::process::id());
        assert!(process.name().is_some_and(|name| !name.is_empty()));
        assert!(peer.runs_this_program);
        assert!(
            peer.parent
                .is_some_and(|parent| parent.pid() != process.pid())
        );
    }
}
