//! Reading a run of git's output, bounded (ADR 0122, ADR 0127): waiting for a child with its
//! standard output and error read on threads of their own, `oso-git-read`, and its input
//! written on another, `oso-git-write`, a run past [`RUN_LIMIT`] killed and left, and the reads'
//! limits.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs on the threads that
//! run git and waits for it.

use std::io::{self, Read, Write};
use std::process::{ChildStdout, ExitStatus};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::process::Child;

use super::{EXIT_POLL, GitError, MAX_STDERR, Output, RUN_LIMIT};

/// Waits for `child`, started with piped standard output and error, writing `input` to its
/// standard input if there is one, and keeping what `read` keeps of its output, which says
/// whether it cut it, and the start of its standard error. A cut run is killed; a run past
/// [`RUN_LIMIT`] is killed and left. `gh` is run this way too (ADR 0121).
pub(crate) fn collect(
    child: Child,
    input: Option<Vec<u8>>,
    read: impl FnOnce(ChildStdout) -> io::Result<(Vec<u8>, bool)> + Send + 'static,
) -> Result<Output, GitError> {
    collect_within(child, input, read, RUN_LIMIT)
}

/// As [`collect`], a run past `within` killed and left: `gh`'s reads of a pull request take
/// 30 s (ADR 0127, step 5a).
pub(crate) fn collect_within(
    mut child: Child,
    input: Option<Vec<u8>>,
    read: impl FnOnce(ChildStdout) -> io::Result<(Vec<u8>, bool)> + Send + 'static,
    within: Duration,
) -> Result<Output, GitError> {
    let deadline = Instant::now() + within;
    let streams = (child.stdout.take(), child.stderr.take());
    let (Some(stdout), Some(stderr)) = streams else {
        stop(&mut child);
        return Err(GitError::Unexpected("no output streams"));
    };
    if let Some(input) = input
        && let Err(e) = write_away(&mut child, input)
    {
        stop(&mut child);
        return Err(e);
    }
    let (sent, got) = mpsc::channel();
    let sent_too = sent.clone();
    let started = reader(move || Stream::Out(read(stdout)), sent).and_then(|()| {
        reader(
            move || Stream::Err(keep_start(stderr, MAX_STDERR)),
            sent_too,
        )
    });
    if let Err(e) = started {
        stop(&mut child);
        return Err(GitError::Io(e));
    }
    let (mut out, mut err) = (None, None);
    while out.is_none() || err.is_none() {
        match got.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Stream::Out(read)) => {
                if matches!(read, Ok((_, true))) {
                    let _ = child.kill();
                }
                out = Some(read);
            }
            Ok(Stream::Err(kept)) => err = Some(kept),
            Err(_) => {
                stop(&mut child);
                return Err(GitError::TimedOut);
            }
        }
    }
    let status = exited(&mut child, deadline)?;
    let (stdout, cut) = out
        .unwrap_or_else(|| Ok((Vec::new(), false)))
        .map_err(GitError::Io)?;
    Ok(Output {
        stdout,
        cut,
        status,
        stderr: String::from_utf8_lossy(&err.unwrap_or_default()).into_owned(),
    })
}

/// What one of a run's reading threads read.
enum Stream {
    Out(io::Result<(Vec<u8>, bool)>),
    Err(Vec<u8>),
}

/// Reads on a thread of its own, sending what it read; a run that is given up on leaves it to
/// end when the stream does.
fn reader(
    read: impl FnOnce() -> Stream + Send + 'static,
    sent: mpsc::Sender<Stream>,
) -> io::Result<()> {
    thread::Builder::new()
        .name("oso-git-read".into())
        .spawn(move || {
            let _ = sent.send(read());
        })
        .map(drop)
}

/// Writes `input` to `child`'s standard input on a thread of its own, and closes it after: a
/// run that is given up on leaves the thread to end when the pipe does.
pub(in crate::git) fn write_away(child: &mut Child, input: Vec<u8>) -> Result<(), GitError> {
    let mut writer = child
        .stdin
        .take()
        .ok_or(GitError::Unexpected("no input stream"))?;
    thread::Builder::new()
        .name("oso-git-write".into())
        .spawn(move || {
            let _ = writer.write_all(&input);
        })
        .map(drop)
        .map_err(GitError::Io)
}

/// Kills `child` and reaps it; one that has gone already is no error.
pub(crate) fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The status of `child`, whose output has ended, once it exits, or `TimedOut` if it is still
/// running at `deadline`, when it is killed.
fn exited(child: &mut Child, deadline: Instant) -> Result<ExitStatus, GitError> {
    loop {
        if let Some(status) = child.try_wait().map_err(GitError::Io)? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            stop(child);
            return Err(GitError::TimedOut);
        }
        thread::sleep(EXIT_POLL);
    }
}

/// Reads `from` to its end, keeping at most `limit` bytes. `true` if there was more.
pub(crate) fn read_at_most(mut from: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    (&mut from).take(cap).read_to_end(&mut kept)?;
    let cut = kept.len() > limit;
    kept.truncate(limit);
    Ok((kept, cut))
}

/// The first `limit` bytes of `from`, reading the rest away so that the writer never waits.
fn keep_start(mut from: impl Read, limit: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut piece = [0; 8192];
    loop {
        match from.read(&mut piece) {
            Ok(0) | Err(_) => return kept,
            Ok(n) => {
                let room = limit.saturating_sub(kept.len()).min(n);
                kept.extend_from_slice(&piece[..room]);
            }
        }
    }
}
