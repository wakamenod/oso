//! Pseudo-terminals, for the terminal (ADR 0120): the master end oso reads and
//! writes, and the slave end a child of [`super::spawn`] runs in.
//!
//! Must not depend on: `control` or `text`, or any idea of what runs in the
//! terminal or how its output is parsed.
//!
//! - Both ends are close-on-exec from the moment they are opened, so no other
//!   child inherits them; the master is non-blocking.
//! - A child runs in a session of its own, with the slave as its controlling
//!   terminal, and the spawn rules of [`super`] hold for it.
//! - Drop the [`Slave`] once the child has started: while oso holds it, the
//!   master never sees the terminal's end.
//! - [`Master::wait`] waits for the master or a [`Waker`], so the thread that
//!   owns a terminal sleeps until there is work.
//!
//! macOS and Linux only for now; elsewhere [`open`] and [`wake_pair`] return
//! [`io::ErrorKind::Unsupported`]. ConPTY will fill `open`, `read`, `write`,
//! `resize` and `wait`; it has no echo flag, process groups or hang-up, so there
//! [`Master::echoes`], [`Master::foreground_group`] and [`Master::hang_up`] will
//! return [`io::ErrorKind::Unsupported`]. A caller treats any error from
//! `echoes` as echo off (a secret), and any error from `foreground_group` as a
//! shell that is not in the foreground.

#[cfg(not(unix))]
mod other;
#[cfg(all(test, unix))]
mod tests;
#[cfg(unix)]
mod unix;

#[cfg(not(unix))]
use other as imp;
#[cfg(unix)]
use unix as imp;

use std::io;
use std::process::Command;
use std::time::Duration;

/// The size of a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub rows: u16,
    pub columns: u16,
    /// The width of the text area in pixels, 0 if not known.
    pub width: u16,
    /// The height of the text area in pixels, 0 if not known.
    pub height: u16,
}

/// The end of a terminal oso reads the child's output from and writes its
/// input to.
#[derive(Debug)]
pub struct Master(imp::Master);

/// The end of a terminal a child runs in, given to [`super::Spawn`].
#[derive(Debug)]
pub struct Slave(imp::Slave);

/// Opens a terminal of `size`.
pub fn open(size: Size) -> io::Result<(Master, Slave)> {
    let (master, slave) = imp::open(size)?;
    Ok((Master(master), Slave(slave)))
}

impl Master {
    /// Reads what the child wrote. Never blocks: with nothing to read, it
    /// returns [`io::ErrorKind::WouldBlock`]. It returns `Ok(0)` once no
    /// process holds the terminal any more. One read returns a few KiB at most,
    /// so a reader reads until `WouldBlock` before it waits again.
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }

    /// Writes input for the child. Never blocks: while the terminal is full, it
    /// returns [`io::ErrorKind::WouldBlock`].
    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    /// Sets the terminal's size, which signals its foreground process group.
    pub fn resize(&self, size: Size) -> io::Result<()> {
        self.0.resize(size)
    }

    /// Whether the terminal echoes what is typed. A password prompt turns echo
    /// off.
    pub fn echoes(&self) -> io::Result<bool> {
        self.0.echoes()
    }

    /// The terminal's foreground process group, `None` if it has none.
    pub fn foreground_group(&self) -> io::Result<Option<u32>> {
        self.0.foreground_group()
    }

    /// Sends `SIGHUP` to the terminal's foreground process group, as closing a
    /// terminal does. Does nothing if it has none.
    pub fn hang_up(&self) -> io::Result<()> {
        self.0.hang_up()
    }

    /// Waits until the master can be read (or has ended), can be written if
    /// `writable` is asked for, or `wakeup` is woken, or `timeout` passes;
    /// forever if it is `None`. Empties `wakeup`. A signal may end the wait
    /// early with nothing ready. A terminal can nearly always be written, so ask
    /// for `writable` only while input waits to be written, or the wait never
    /// sleeps.
    pub fn wait(
        &self,
        wakeup: &Wakeup,
        writable: bool,
        timeout: Option<Duration>,
    ) -> io::Result<Ready> {
        self.0.wait(&wakeup.0, writable, timeout)
    }
}

/// Wakes the thread that waits in [`Master::wait`].
#[derive(Debug, Clone)]
pub struct Waker(imp::Waker);

/// What [`Master::wait`] watches beside the master.
#[derive(Debug)]
pub struct Wakeup(imp::Wakeup);

/// A [`Waker`] and the [`Wakeup`] it wakes.
pub fn wake_pair() -> io::Result<(Waker, Wakeup)> {
    let (waker, wakeup) = imp::wake_pair()?;
    Ok((Waker(waker), Wakeup(wakeup)))
}

impl Waker {
    /// Wakes the waiting thread. Never blocks, and costs nothing while a wake is
    /// already pending: wakes not yet seen are one.
    pub fn wake(&self) -> io::Result<()> {
        self.0.wake()
    }
}

/// What [`Master::wait`] found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ready {
    /// The master can be read, or has ended.
    pub readable: bool,
    /// The master can be written.
    pub writable: bool,
    /// The [`Waker`] was woken.
    pub woken: bool,
}

/// Makes `slave` the standard streams and the controlling terminal of the
/// child `command` starts.
pub(super) fn run_in(slave: &Slave, command: &mut Command) -> io::Result<()> {
    imp::run_in(&slave.0, command)
}
