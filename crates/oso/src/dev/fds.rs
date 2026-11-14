//! Descriptors that go with a dev socket request or its reply, for `oso dev`'s restart of
//! terminals (ADR 0120, step 11): each PTY's master goes from the instance going to the
//! supervisor in a reply, and from it to the new instance in a request, with `SCM_RIGHTS`.
//!
//! Must not depend on: `app`, editor state or the protocol's methods; this moves descriptors
//! and lines, and the caller judges what they are.
//!
//! - A request may bring at most `ControlStream::MAX_FDS`. Only `import-terminals` takes
//!   them; with any other request they are closed as it is answered.
//! - Nothing here reads what the descriptors refer to.

use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use oso_platform::control::ControlDir;
#[cfg(unix)]
use oso_platform::control::ControlStream;
use serde_json::Value;

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
        self.stream.set_read_timeout(timeout)
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

/// Sends `method` with `params` to the instance `pid`, with `fds` along, and returns its
/// response line and the descriptors that came with it, giving up after `patience`.
#[cfg(unix)]
pub fn exchange(
    dir: &ControlDir,
    pid: u32,
    method: &str,
    params: Option<Value>,
    fds: &[Fd],
    patience: Duration,
) -> io::Result<(String, Vec<Fd>)> {
    use oso_platform::control::Socket;

    use super::protocol::{self, MAX_LINE};
    let (_, mut stream) = dir.connect(Socket::Dev, Some(pid))?;
    stream.set_read_timeout(Some(patience))?;
    stream.set_write_timeout(Some(patience))?;
    write_line(&mut stream, &protocol::request(1, method, params), fds)?;
    let mut reader = io::BufReader::new(Receiving::new(stream));
    let mut response = Vec::new();
    crate::channel::wire::read_line_within(&mut reader, &mut response, patience)?;
    if response.last() != Some(&b'\n') || response.len() > MAX_LINE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the response is missing, incomplete or too long",
        ));
    }
    let fds = reader.get_mut().take_fds();
    let line = String::from_utf8(response)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the response is not UTF-8"))?;
    Ok((line.trim_end().to_owned(), fds))
}

/// Where descriptors cannot pass, nothing is exchanged.
#[cfg(not(unix))]
pub fn exchange(
    _dir: &ControlDir,
    _pid: u32,
    _method: &str,
    _params: Option<Value>,
    _fds: &[Fd],
    _patience: Duration,
) -> io::Result<(String, Vec<Fd>)> {
    Err(io::ErrorKind::Unsupported.into())
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
