//! The control socket where it is not implemented yet (Windows, ADR 0031).
//! The API matches the Unix one, and every operation fails.

use std::convert::Infallible;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "the dev control socket is not supported on this system yet",
    )
}

/// The private directory that holds control sockets. Never constructed
/// here.
#[derive(Debug, Clone)]
pub struct ControlDir {
    never: Infallible,
}

impl ControlDir {
    /// Fails: not supported on this system.
    pub fn open() -> io::Result<Self> {
        Err(unsupported())
    }

    /// Fails: not supported on this system.
    pub fn open_existing() -> io::Result<Self> {
        Err(unsupported())
    }

    /// Fails: not supported on this system.
    pub fn at(_path: impl Into<PathBuf>) -> io::Result<Self> {
        Err(unsupported())
    }

    /// Unreachable: no `ControlDir` exists.
    pub fn path(&self) -> &Path {
        match self.never {}
    }

    /// Unreachable: no `ControlDir` exists.
    pub fn bind(&self) -> io::Result<ControlListener> {
        match self.never {}
    }

    /// Unreachable: no `ControlDir` exists.
    pub fn connect(&self, _pid: Option<u32>) -> io::Result<(u32, ControlStream)> {
        match self.never {}
    }

    /// Unreachable: no `ControlDir` exists.
    pub fn remove_stale(&self) {
        match self.never {}
    }

    /// Unreachable: no `ControlDir` exists.
    pub fn write_private_file(&self, _name: &str, _bytes: &[u8]) -> io::Result<PathBuf> {
        match self.never {}
    }
}

/// A listening control socket. Never constructed here.
#[derive(Debug)]
pub struct ControlListener {
    never: Infallible,
}

impl ControlListener {
    /// Unreachable: no `ControlListener` exists.
    pub fn socket_path(&self) -> PathBuf {
        match self.never {}
    }

    /// Unreachable: no `ControlListener` exists.
    pub fn accept(&self) -> io::Result<ControlStream> {
        match self.never {}
    }

    /// Unreachable: no `ControlListener` exists.
    pub fn cleanup_guard(&self) -> CleanupGuard {
        match self.never {}
    }
}

/// Removes this process's control files when dropped. Never constructed
/// here.
#[derive(Debug, Clone)]
pub struct CleanupGuard {
    never: Infallible,
}

/// A control connection. Never constructed here.
#[derive(Debug)]
pub struct ControlStream {
    never: Infallible,
}

impl ControlStream {
    /// Unreachable: no `ControlStream` exists.
    pub fn try_clone(&self) -> io::Result<Self> {
        match self.never {}
    }

    /// Unreachable: no `ControlStream` exists.
    pub fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        match self.never {}
    }

    /// Unreachable: no `ControlStream` exists.
    pub fn set_write_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
        match self.never {}
    }

    /// Fails: not supported on this system.
    #[doc(hidden)]
    pub fn pair() -> io::Result<(Self, Self)> {
        Err(unsupported())
    }
}

impl Read for ControlStream {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        match self.never {}
    }
}

impl Write for ControlStream {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        match self.never {}
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.never {}
    }
}
