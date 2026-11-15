//! The dev socket's exchange with descriptors, for `oso dev`'s restart of terminals (ADR 0120,
//! step 11): the supervisor takes each PTY's master from the instance going in a reply, and
//! gives it to the new instance in a request. The descriptors and lines move as in
//! `carry::fds`, which the pair of `oso-restart` uses too.
//!
//! Must not depend on: `app`, editor state or the protocol's methods.

use std::io;
use std::time::Duration;

use oso_platform::control::ControlDir;
use serde_json::Value;

pub use crate::carry::fds::{Fd, Passing};
#[cfg(unix)]
pub use crate::carry::fds::{Receiving, write_line};

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
