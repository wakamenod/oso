//! Pseudo-terminals where oso has none yet: Windows, until ConPTY (ADR 0120).
//! Nothing can be opened, so no value of these types exists.
//!
//! Must not depend on: `control` or `text`.

use std::io;
use std::process::Command;
use std::time::Duration;

use super::{Ready, Size};

/// A type with no values.
#[derive(Debug, Clone, Copy)]
enum Never {}

#[derive(Debug)]
pub(super) struct Master(Never);

#[derive(Debug)]
pub(super) struct Slave(Never);

#[derive(Debug, Clone)]
pub(super) struct Waker(Never);

#[derive(Debug)]
pub(super) struct Wakeup(Never);

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "terminals are not supported on this system yet",
    )
}

pub(super) fn open(_size: Size) -> io::Result<(Master, Slave)> {
    Err(unsupported())
}

pub(super) fn wake_pair() -> io::Result<(Waker, Wakeup)> {
    Err(unsupported())
}

impl Master {
    pub(super) fn read(&self, _buf: &mut [u8]) -> io::Result<usize> {
        match self.0 {}
    }

    pub(super) fn write(&self, _buf: &[u8]) -> io::Result<usize> {
        match self.0 {}
    }

    pub(super) fn resize(&self, _size: Size) -> io::Result<()> {
        match self.0 {}
    }

    pub(super) fn reads_secret(&self) -> io::Result<bool> {
        match self.0 {}
    }

    pub(super) fn foreground_group(&self) -> io::Result<Option<u32>> {
        match self.0 {}
    }

    pub(super) fn hang_up(&self) -> io::Result<()> {
        match self.0 {}
    }

    pub(super) fn wait(
        &self,
        _wakeup: &Wakeup,
        _writable: bool,
        _timeout: Option<Duration>,
    ) -> io::Result<Ready> {
        match self.0 {}
    }
}

impl Waker {
    pub(super) fn wake(&self) -> io::Result<()> {
        match self.0 {}
    }
}

pub(super) fn run_in(slave: &Slave, _command: &mut Command) -> io::Result<()> {
    match slave.0 {}
}
