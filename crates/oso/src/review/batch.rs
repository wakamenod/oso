//! Reading blobs for a review through one long-lived `git cat-file --batch` (ADR 0122), fed
//! object ids on its standard input.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this waits for git on the
//! review's thread.
//!
//! - It starts with the first read and lives as long as the review's thread. A thread of its
//!   own reads its answers, so that a read past `git::RUN_LIMIT` is given up on whatever holds
//!   the output open: the process is killed and the next read starts another.
//! - A blob past [`MAX_BLOB`] is not read: the process is killed and the next read starts
//!   another, as reading its bytes away could take long.

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Instant;

use oso_platform::process::{Sink, Source};

use super::MAX_BLOB;
use super::git::{self, Git, GitError};
use super::id::{Id, trim_line};

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

/// One answer as the reading thread parsed it: the id its header named, and what followed.
type Answer = Result<(Box<[u8]>, Blob), GitError>;

/// The running `cat-file`.
struct Running {
    child: Child,
    input: ChildStdin,
    answers: Receiver<Answer>,
}

impl Drop for Running {
    fn drop(&mut self) {
        git::stop(&mut self.child);
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
        let asked = writeln!(running.input, "{}", id.as_str()).and_then(|()| running.input.flush());
        if let Err(e) = asked {
            self.running = None;
            return Err(GitError::Io(e));
        }
        let deadline = Instant::now() + git::RUN_LIMIT;
        let answer = running
            .answers
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Err(GitError::TimedOut));
        match answer {
            Ok((named, blob)) if &*named == id.as_str().as_bytes() => {
                if matches!(blob, Blob::TooLarge(_)) {
                    self.running = None;
                }
                Ok(blob)
            }
            Ok(_) => {
                self.running = None;
                Err(GitError::Unexpected("a cat-file answer for another object"))
            }
            Err(e) => {
                self.running = None;
                Err(e)
            }
        }
    }
}

fn start(git: &Git) -> Result<Running, GitError> {
    let mut child = git.start(
        &[OsStr::new("cat-file"), OsStr::new("--batch")],
        &[],
        Source::Piped,
        // Nothing reads its standard error, and a closed one would end it at its first warning.
        Sink::Null,
    )?;
    let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
        git::stop(&mut child);
        return Err(GitError::Unexpected("no streams"));
    };
    let (sent, answers) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("oso-review-blobs".into())
        .spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                let answer = next(&mut output);
                let last = !matches!(answer, Ok((_, Blob::Bytes(_) | Blob::Missing)));
                if sent.send(answer).is_err() || last {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        git::stop(&mut child);
        return Err(GitError::Io(e));
    }
    Ok(Running {
        child,
        input,
        answers,
    })
}

/// Reads one answer from `output`: a header, then a blob's bytes unless it is too large.
fn next(output: &mut BufReader<ChildStdout>) -> Answer {
    let mut header = Vec::new();
    output
        .take(MAX_HEADER)
        .read_until(b'\n', &mut header)
        .map_err(GitError::Io)?;
    if header.is_empty() {
        return Err(GitError::Unexpected("cat-file ended"));
    }
    let header = trim_line(&header);
    let mut words = header.split(|&byte| byte == b' ');
    let (Some(named), Some(kind)) = (words.next(), words.next()) else {
        return Err(GitError::Unexpected("a cat-file header"));
    };
    if kind == b"missing" {
        return Ok((named.into(), Blob::Missing));
    }
    let size = words
        .next()
        .and_then(|size| std::str::from_utf8(size).ok()?.parse::<u64>().ok())
        .ok_or(GitError::Unexpected("a cat-file size"))?;
    if kind != b"blob" {
        return Err(GitError::Unexpected("an object that is no blob"));
    }
    if size > MAX_BLOB {
        return Ok((named.into(), Blob::TooLarge(size)));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size + 1).unwrap_or(0));
    output
        .take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(GitError::Io)?;
    if bytes.pop() != Some(b'\n') || bytes.len() as u64 != size {
        return Err(GitError::Unexpected("a cat-file body"));
    }
    Ok((named.into(), Blob::Bytes(bytes)))
}
