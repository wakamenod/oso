//! Descriptors that go with a restart's request or its reply (ADR 0120, step 11; ADR 0133):
//! each PTY's master goes from the instance going to the one coming with `SCM_RIGHTS`, through
//! `oso dev`'s supervisor over the dev socket, or straight over `oso-restart`'s pair.
//!
//! Must not depend on: `app`, editor state or the methods; this moves descriptors and lines,
//! and the caller judges what they are.
//!
//! - A request may bring at most `ControlStream::MAX_FDS`. Only the one that hands the masters
//!   over takes them; with any other request they are closed as it is answered.
//! - Nothing here reads what the descriptors refer to.

use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use oso_platform::control::ControlStream;

/// A descriptor passed over the socket; there are none where descriptors cannot pass.
#[cfg(unix)]
pub type Fd = std::os::fd::OwnedFd;
#[cfg(not(unix))]
pub type Fd = std::convert::Infallible;

/// Another descriptor for what `fd` refers to, close-on-exec.
pub fn try_clone(fd: &Fd) -> io::Result<Fd> {
    #[cfg(unix)]
    return fd.try_clone();
    #[cfg(not(unix))]
    match *fd {}
}

/// The descriptors a reply takes, which the UI thread puts and the client's thread sends.
#[derive(Clone, Default)]
pub struct Passing(Arc<Mutex<Vec<Fd>>>);

impl Passing {
    /// Puts `fds` to go with the reply, after any already put.
    pub fn put(&self, fds: Vec<Fd>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(fds);
    }

    /// Takes what was put.
    pub fn take(&self) -> Vec<Fd> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl fmt::Debug for Passing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.0.lock().unwrap_or_else(PoisonError::into_inner).len();
        write!(f, "Passing({count})")
    }
}

/// A connection read through `recvmsg`, keeping the descriptors that come with its bytes.
#[cfg(unix)]
pub struct Receiving {
    stream: ControlStream,
    fds: Vec<Fd>,
}

#[cfg(unix)]
impl Receiving {
    pub fn new(stream: ControlStream) -> Self {
        Self {
            stream,
            fds: Vec::new(),
        }
    }

    /// The descriptors read so far.
    pub fn take_fds(&mut self) -> Vec<Fd> {
        std::mem::take(&mut self.fds)
    }
}

#[cfg(unix)]
impl io::Read for Receiving {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.recv_with_fds(buf, &mut self.fds)
    }
}

#[cfg(unix)]
impl crate::channel::wire::ReadTimeout for Receiving {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self.stream.set_read_timeout(timeout) {
            // macOS refuses a timeout on a connection the other end has closed; the read that
            // follows then finds its end at once, which is what the caller waits for.
            Err(e)
                if e.kind() == io::ErrorKind::InvalidInput
                    && timeout.is_some_and(|t| !t.is_zero()) =>
            {
                Ok(())
            }
            done => done,
        }
    }
}

/// Writes `line` and a newline, with `fds` sent along with it if there are any.
#[cfg(unix)]
pub fn write_line(stream: &mut ControlStream, line: &str, fds: &[Fd]) -> io::Result<()> {
    use std::io::Write;
    use std::os::fd::AsFd;
    if fds.is_empty() {
        return writeln!(stream, "{line}");
    }
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    let borrowed: Vec<_> = fds.iter().map(AsFd::as_fd).collect();
    stream.send_with_fds(&bytes, &borrowed)
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufReader, Write};

    use oso_platform::control::ControlStream;

    use super::{Passing, Receiving, write_line};
    use crate::channel::wire::read_line_within;

    #[test]
    fn a_line_brings_its_descriptors_and_the_next_brings_none() {
        let (mut a, b) = ControlStream::pair().unwrap();
        let (_reader, writer) = std::io::pipe().unwrap();
        let passing = Passing::default();
        passing.put(vec![writer.try_clone().unwrap().into()]);
        write_line(&mut a, "{\"first\": 1}", &passing.take()).unwrap();
        a.write_all(b"{\"second\": 2}\n").unwrap();
        let mut reader = BufReader::new(Receiving::new(b));
        let mut line = Vec::new();
        read_line_within(&mut reader, &mut line, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(line, b"{\"first\": 1}\n");
        assert_eq!(reader.get_mut().take_fds().len(), 1);
        line.clear();
        read_line_within(&mut reader, &mut line, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(line, b"{\"second\": 2}\n");
        assert!(reader.get_mut().take_fds().is_empty());
    }
}
