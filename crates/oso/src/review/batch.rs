//! Reading blobs for a review through one long-lived `git cat-file --batch` (ADR 0122), fed
//! object ids on its standard input.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this waits for git on the
//! review's thread.
//!
//! - It starts with the first read and lives as long as the review's thread.
//! - A blob past [`MAX_BLOB`] is not read: the process is killed, and the next read starts
//!   another, as reading its bytes away could take long.
//! - A read past `git::RUN_LIMIT` kills it the same way, and says so.

use std::ffi::OsStr;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::{Arc, Mutex};

use oso_platform::process::Source;

use super::git::{self, Git, GitError, Id};

/// The largest blob read: a side of a diff is at most this.
pub(crate) const MAX_BLOB: u64 = 8 * 1024 * 1024;
/// The longest header line `cat-file` writes for one object.
const MAX_HEADER: u64 = 256;

/// What a read found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Blob {
    Bytes(Vec<u8>),
    /// A blob of this many bytes, more than [`MAX_BLOB`].
    TooLarge(u64),
    /// No blob by that id.
    Missing,
}

/// The running `cat-file`.
struct Running {
    child: Arc<Mutex<Child>>,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Drop for Running {
    fn drop(&mut self) {
        git::kill(&self.child);
        let _ = git::lock(&self.child).wait();
    }
}

/// Blobs by id, read in turn.
#[derive(Default)]
pub(crate) struct Batch {
    running: Option<Running>,
}

impl Batch {
    /// The blob `id`, through the running `cat-file`, started first if none runs.
    pub(crate) fn read(&mut self, git: &Git, id: &Id) -> Result<Blob, GitError> {
        let running = match &mut self.running {
            Some(running) => running,
            None => self.running.insert(start(git)?),
        };
        let child = Arc::clone(&running.child);
        let (read, killed) = git::watched(&child, git::RUN_LIMIT, || ask(running, id));
        if killed {
            self.running = None;
            return Err(GitError::TimedOut);
        }
        match read {
            Ok(Answer::Done(blob)) => Ok(blob),
            Ok(Answer::DoneAndStop(blob)) => {
                self.running = None;
                Ok(blob)
            }
            Err(e) => {
                self.running = None;
                Err(e)
            }
        }
    }
}

/// A read's answer, and whether the process must stop after it.
enum Answer {
    Done(Blob),
    DoneAndStop(Blob),
}

fn start(git: &Git) -> Result<Running, GitError> {
    let mut child = git.start(
        &[OsStr::new("cat-file"), OsStr::new("--batch")],
        &[],
        Source::Piped,
    )?;
    // Nothing reads its standard error, so it must not be able to fill.
    drop(child.stderr.take());
    let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(GitError::Unexpected("no streams"));
    };
    Ok(Running {
        child: Arc::new(Mutex::new(child)),
        input,
        output: BufReader::new(output),
    })
}

/// Asks for `id` and reads its answer.
fn ask(running: &mut Running, id: &Id) -> Result<Answer, GitError> {
    let broken = |e: io::Error| GitError::Spawn(e);
    writeln!(running.input, "{}", id.as_str()).map_err(broken)?;
    running.input.flush().map_err(broken)?;
    let mut header = Vec::new();
    (&mut running.output)
        .take(MAX_HEADER)
        .read_until(b'\n', &mut header)
        .map_err(broken)?;
    let header = git::trim_line(&header);
    let mut words = header.split(|&byte| byte == b' ');
    let (Some(named), Some(kind)) = (words.next(), words.next()) else {
        return Err(GitError::Unexpected("a cat-file header"));
    };
    if named != id.as_str().as_bytes() {
        return Err(GitError::Unexpected("a cat-file header for another object"));
    }
    if kind == b"missing" {
        return Ok(Answer::Done(Blob::Missing));
    }
    let size = words
        .next()
        .and_then(|size| std::str::from_utf8(size).ok()?.parse::<u64>().ok())
        .ok_or(GitError::Unexpected("a cat-file size"))?;
    if kind != b"blob" {
        // Not a blob, as no side is: the process stops rather than read it away.
        return Ok(Answer::DoneAndStop(Blob::Missing));
    }
    if size > MAX_BLOB {
        return Ok(Answer::DoneAndStop(Blob::TooLarge(size)));
    }
    let mut bytes = Vec::new();
    (&mut running.output)
        .take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(broken)?;
    if bytes.pop() != Some(b'\n') || bytes.len() as u64 != size {
        return Err(GitError::Unexpected("a cat-file body"));
    }
    Ok(Answer::Done(Blob::Bytes(bytes)))
}
