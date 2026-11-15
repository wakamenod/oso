//! A process [`super::spawn`] started: its pipes, and waiting for it or ending it (ADR 0137).
//!
//! Must not depend on: `control` or `text`, or what the child runs.
//!
//! It has the standard library's `Child` methods that oso uses, with its pipe and status types,
//! as the spawn on Unix is oso's own (`process::unix`) and the standard library makes a `Child`
//! only from its own spawn. As with the standard library's, dropping it neither waits for the
//! process nor ends it.

use std::io::{self, Read};
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus, Output};

use super::{Signal, imp};

/// A process [`super::spawn`] started.
#[derive(Debug)]
pub struct Child {
    /// Its standard input, if [`super::Source::Piped`].
    pub stdin: Option<ChildStdin>,
    /// Its standard output, if [`super::Sink::Piped`].
    pub stdout: Option<ChildStdout>,
    /// Its standard error, if [`super::Sink::Piped`].
    pub stderr: Option<ChildStderr>,
    pub(super) process: imp::Process,
}

impl Child {
    /// The process's ID.
    pub fn id(&self) -> u32 {
        self.process.id()
    }

    /// Waits for the process to end, and says how it ended. Its standard input, if piped and
    /// still here, is closed first, so that a child that reads to the end can end.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        self.process.wait()
    }

    /// How the process ended, or `None` while it runs. Never waits.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.process.try_wait()
    }

    /// Ends the process at once (`SIGKILL` on Unix). Does nothing once it has been waited
    /// for, as its ID may be another process's by then.
    pub fn kill(&mut self) -> io::Result<()> {
        self.process.kill()
    }

    /// Asks the process to end (`SIGTERM` on Unix), so that it can clean up first, as git
    /// takes its lock files away. Windows has no such signal: there it ends at once. Does
    /// nothing once it has been waited for.
    pub fn terminate(&mut self) -> io::Result<()> {
        self.process.signal(Signal::Terminate, false)
    }

    /// Sends `signal` to the process group the process leads, as one started with
    /// [`super::Spawn::own_process_group`] does on Unix, so that what it started gets it too,
    /// unless that moved to a group of its own. Refused for one that leads no group. Windows
    /// has no group here: the process alone ends at once. Does nothing once the process has
    /// been waited for, as its group's ID may be another's by then, so what it left running
    /// is not reached.
    pub fn signal_group(&mut self, signal: Signal) -> io::Result<()> {
        self.process.signal(signal, true)
    }

    /// Closes the standard input, reads the standard output and error to their ends at the same
    /// time, so that a child that fills one while the other is read never stalls, and waits.
    /// With both piped, the standard error is read on a thread of its own.
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        drop(self.stdin.take());
        let (stdout, stderr) = (self.stdout.take(), self.stderr.take());
        let read = std::thread::scope(|scope| {
            let stderr = stderr
                .map(|stderr| {
                    std::thread::Builder::new()
                        .name("oso-child-stderr".into())
                        .spawn_scoped(scope, move || read_all(Some(stderr)))
                })
                .transpose()?;
            let stdout = read_all(stdout);
            let stderr = match stderr {
                Some(reader) => reader
                    .join()
                    .unwrap_or_else(|_| Err(io::Error::other("the reader of stderr panicked"))),
                None => Ok(Vec::new()),
            };
            Ok::<_, io::Error>((stdout?, stderr?))
        });
        // Waited for even when a read failed, so that no child is left unreaped.
        let status = self.wait()?;
        let (stdout, stderr) = read?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

/// All that `stream` holds, nothing if there is none.
fn read_all(stream: Option<impl Read>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(mut stream) = stream {
        stream.read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}
