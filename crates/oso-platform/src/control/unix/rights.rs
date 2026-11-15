//! File descriptors passed over a control connection with `SCM_RIGHTS`, for a restart that
//! carries terminals: `oso dev`'s supervisor carries each terminal's PTY from the instance it
//! replaces to the new one (ADR 0120, step 11), and `oso-restart` sends them over the pair the
//! two instances share (ADR 0133).
//!
//! Must not depend on: `text`, the protocol spoken over the socket, or what the descriptors
//! are; the caller judges them.
//!
//! - Descriptors go with the first byte of what [`ControlStream::send_with_fds`] writes, and
//!   arrive with the read that returns that byte.
//! - A read takes at most [`ControlStream::MAX_FDS`] in all. One that would bring more, or
//!   whose control data the system cut short, closes every descriptor it brought and fails.
//! - Each descriptor received is close-on-exec: at once on Linux (`MSG_CMSG_CLOEXEC`), right
//!   after the read on macOS, which has no such flag. No child of oso's sees one meanwhile, as
//!   each gets only the descriptors it is given (ADR 0137).

use std::io::{self, Write};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

use super::ControlStream;

/// The bytes of one descriptor in the control data.
const FD_SIZE: usize = size_of::<RawFd>();

/// Room for the control data of [`ControlStream::MAX_FDS`] descriptors and their header,
/// in words, so that it is aligned for `cmsghdr`: 512 bytes, more than either system needs.
const CONTROL_WORDS: usize = 64;

/// Flags for `sendmsg`: no `SIGPIPE` where the system can be told so per call.
#[cfg(any(target_os = "linux", target_os = "android"))]
const SEND_FLAGS: libc::c_int = libc::MSG_NOSIGNAL;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const SEND_FLAGS: libc::c_int = 0;

/// Flags for `recvmsg`: descriptors close-on-exec from the start where the system can.
#[cfg(any(target_os = "linux", target_os = "android"))]
const RECEIVE_FLAGS: libc::c_int = libc::MSG_CMSG_CLOEXEC;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const RECEIVE_FLAGS: libc::c_int = 0;

impl ControlStream {
    /// The most descriptors one message carries, and one read takes.
    pub const MAX_FDS: usize = 64;

    /// Writes all of `bytes`, with `fds` sent along with the first. `bytes` must not be
    /// empty, and `fds` holds at most [`Self::MAX_FDS`]. The descriptors stay open here: the
    /// peer gets its own.
    pub fn send_with_fds(&self, bytes: &[u8], fds: &[BorrowedFd<'_>]) -> io::Result<()> {
        if bytes.is_empty() || fds.len() > Self::MAX_FDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "descriptors need a byte to go with, and at most 64 go at once",
            ));
        }
        let raw: Vec<RawFd> = fds.iter().map(AsRawFd::as_raw_fd).collect();
        let payload = libc::c_uint::try_from(raw.len() * FD_SIZE).map_err(io::Error::other)?;
        // SAFETY: `CMSG_SPACE` only computes a size from its argument.
        let space = unsafe { libc::CMSG_SPACE(payload) };
        // SAFETY: `CMSG_LEN` only computes a size from its argument.
        let len = unsafe { libc::CMSG_LEN(payload) };
        let mut control = [0u64; CONTROL_WORDS];
        let space = usize::try_from(space).map_err(io::Error::other)?;
        if space > size_of_val(&control) {
            return Err(io::Error::other("the control data does not fit"));
        }
        let mut iov = libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        };
        // SAFETY: `msghdr` is plain data, for which all zeros is an empty header.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        if !raw.is_empty() {
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = space as _;
            // SAFETY: `msg_control` points at `control`, which holds `msg_controllen` bytes
            // aligned for `cmsghdr`, so the first header is inside it or null.
            let header = unsafe { libc::CMSG_FIRSTHDR(&raw const msg) };
            if header.is_null() {
                return Err(io::Error::other("no room for the control header"));
            }
            // SAFETY: `header` is non-null and inside `control` with room for a whole
            // `cmsghdr` and its data, as `space` was checked against its size; it is
            // aligned for `cmsghdr`, as `control` is.
            unsafe {
                header.write(libc::cmsghdr {
                    cmsg_len: len as _,
                    cmsg_level: libc::SOL_SOCKET,
                    cmsg_type: libc::SCM_RIGHTS,
                });
            }
            // SAFETY: `header` is a valid header inside `control`; this only computes where
            // its data starts.
            let data = unsafe { libc::CMSG_DATA(header) };
            // SAFETY: `data` has room for `raw.len()` descriptors inside `control`, as
            // `CMSG_SPACE` said, and `raw` is a separate allocation of that many.
            unsafe {
                std::ptr::copy_nonoverlapping(raw.as_ptr().cast::<u8>(), data, raw.len() * FD_SIZE);
            }
        }
        let sent = loop {
            // SAFETY: `msg` points at `iov` and `control`, which outlive the call and hold
            // what the header says; the socket is this stream's, kept open by `self`.
            let n = unsafe { libc::sendmsg(self.0.as_raw_fd(), &raw const msg, SEND_FLAGS) };
            if let Ok(n) = usize::try_from(n) {
                break n;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        };
        if sent == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        // The descriptors went with the first byte; the rest goes without.
        (&self.0).write_all(&bytes[sent..])
    }

    /// Reads into `buf` as a read does, and appends the descriptors that came with those
    /// bytes to `fds`, each close-on-exec. Fails, and closes what came, if `fds` would then
    /// hold more than [`Self::MAX_FDS`], or the system cut the control data short.
    pub fn recv_with_fds(&self, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<usize> {
        let mut control = [0u64; CONTROL_WORDS];
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        // SAFETY: `msghdr` is plain data, for which all zeros is an empty header.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = size_of_val(&control) as _;
        let (read, received) = loop {
            // SAFETY: `msg` points at `iov`, over `buf`, and at `control`, which outlive the
            // call and hold the lengths the header says; the socket is kept open by `self`.
            let n = unsafe { libc::recvmsg(self.0.as_raw_fd(), &raw mut msg, RECEIVE_FLAGS) };
            if let Ok(n) = usize::try_from(n) {
                let received = received(&msg);
                #[cfg(not(any(target_os = "linux", target_os = "android")))]
                for fd in &received {
                    close_on_exec(fd)?;
                }
                break (n, received);
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => {}
                _ => return Err(error),
            }
        };
        if msg.msg_flags & libc::MSG_CTRUNC != 0 || fds.len() + received.len() > Self::MAX_FDS {
            // Dropping them closes them.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "more descriptors came than a read takes",
            ));
        }
        fds.extend(received);
        Ok(read)
    }
}

/// The descriptors in the control data `recvmsg` filled in `msg`, each owned from now on.
fn received(msg: &libc::msghdr) -> Vec<OwnedFd> {
    let mut fds = Vec::new();
    // SAFETY: `msg` is the header `recvmsg` filled, whose control data lies in the buffer it
    // points at; this finds the first header in it, or null.
    let mut header = unsafe { libc::CMSG_FIRSTHDR(msg) };
    while !header.is_null() {
        // SAFETY: `header` is non-null, so `CMSG_FIRSTHDR` or `CMSG_NXTHDR` found a whole
        // header inside the control data the system wrote.
        let head = unsafe { header.read_unaligned() };
        if head.cmsg_level == libc::SOL_SOCKET && head.cmsg_type == libc::SCM_RIGHTS {
            // SAFETY: `header` is a valid header; this only computes where its data starts.
            let data = unsafe { libc::CMSG_DATA(header) };
            let offset = data as usize - header as usize;
            // A header the system cut short may still claim its whole length: count only what
            // lies inside the control data it returned.
            let returned =
                (msg.msg_control as usize + control_len(msg)).saturating_sub(data as usize);
            let claimed = (head.cmsg_len as usize).saturating_sub(offset);
            let count = claimed.min(returned) / FD_SIZE;
            for at in 0..count {
                let from = data.wrapping_add(at * FD_SIZE).cast::<RawFd>();
                // SAFETY: `count` descriptors follow `data`, by the header's length and inside
                // the control data the system returned.
                let raw = unsafe { from.read_unaligned() };
                // SAFETY: the system has just installed `raw` in this process for this read,
                // and nothing else owns it.
                fds.push(unsafe { OwnedFd::from_raw_fd(raw) });
            }
        }
        // SAFETY: `msg` and `header` are valid as above; this finds the next header, or null.
        header = unsafe { libc::CMSG_NXTHDR(msg, header) };
    }
    fds
}

/// The length of the control data `recvmsg` returned in `msg`: a `size_t` on Linux, a
/// `socklen_t` elsewhere.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn control_len(msg: &libc::msghdr) -> usize {
    msg.msg_controllen
}

/// The length of the control data `recvmsg` returned in `msg`: a `size_t` on Linux, a
/// `socklen_t` elsewhere.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn control_len(msg: &libc::msghdr) -> usize {
    msg.msg_controllen as usize
}

/// Marks `fd` close-on-exec, where the read could not.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn close_on_exec(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: `F_SETFD` sets the descriptor flags of `fd`, which `fd` keeps open, and touches
    // no memory.
    let done = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    if done == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::fd::{AsFd, AsRawFd, OwnedFd};

    use super::ControlStream;

    fn cloexec(fd: &OwnedFd) -> bool {
        // SAFETY: `F_GETFD` reads the flags of a descriptor `fd` keeps open.
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
        flags >= 0 && flags & libc::FD_CLOEXEC != 0
    }

    #[test]
    fn a_descriptor_goes_with_the_first_byte_and_works_on_the_other_side() {
        let (a, b) = ControlStream::pair().unwrap();
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        a.send_with_fds(b"take this\n", &[writer.as_fd()]).unwrap();
        let mut fds = Vec::new();
        let mut buf = [0u8; 64];
        let n = b.recv_with_fds(&mut buf, &mut fds).unwrap();
        assert_eq!(&buf[..n], b"take this\n");
        assert_eq!(fds.len(), 1);
        assert!(cloexec(&fds[0]), "received descriptors are close-on-exec");
        // The copy that came writes into the same pipe.
        let mut theirs = std::fs::File::from(fds.pop().unwrap());
        theirs.write_all(b"x").unwrap();
        writer.write_all(b"y").unwrap();
        drop((theirs, writer));
        let mut got = String::new();
        reader.read_to_string(&mut got).unwrap();
        assert_eq!(got, "xy");
    }

    #[test]
    fn a_plain_read_on_the_other_side_gets_the_bytes_and_no_descriptor() {
        let (a, mut b) = ControlStream::pair().unwrap();
        let (_reader, writer) = std::io::pipe().unwrap();
        a.send_with_fds(b"hi", &[writer.as_fd()]).unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(b.read(&mut buf).unwrap(), 2);
    }

    #[test]
    fn more_than_a_read_takes_is_refused_and_closed() {
        let (a, b) = ControlStream::pair().unwrap();
        let (_reader, writer) = std::io::pipe().unwrap();
        a.send_with_fds(b"a", &[writer.as_fd(), writer.as_fd()])
            .unwrap();
        let mut fds: Vec<OwnedFd> = (0..ControlStream::MAX_FDS - 1)
            .map(|_| writer.try_clone().unwrap().into())
            .collect();
        let mut buf = [0u8; 8];
        assert!(b.recv_with_fds(&mut buf, &mut fds).is_err());
        assert_eq!(fds.len(), ControlStream::MAX_FDS - 1, "nothing was added");
    }

    #[test]
    fn nothing_goes_without_a_byte_or_past_the_most() {
        let (a, _b) = ControlStream::pair().unwrap();
        let (_reader, writer) = std::io::pipe().unwrap();
        assert!(a.send_with_fds(b"", &[writer.as_fd()]).is_err());
        let many = vec![writer.as_fd(); ControlStream::MAX_FDS + 1];
        assert!(a.send_with_fds(b"a", &many).is_err());
        let most = vec![writer.as_fd(); ControlStream::MAX_FDS];
        a.send_with_fds(b"a", &most).unwrap();
    }
}
