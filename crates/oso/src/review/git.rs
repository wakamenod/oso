//! Running `git` for a review (ADR 0122): oso's own arguments and environment, every read
//! bounded, and a run past [`RUN_LIMIT`] killed.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs on the review's
//! thread and waits for git.
//!
//! - Every run starts `git --no-pager -c core.quotePath=false`, in the work tree's top, and
//!   gets the process module's allowlist plus `GIT_OPTIONAL_LOCKS=0` (a read takes no lock
//!   from the agent's own git), `GIT_TERMINAL_PROMPT=0` and, by name, `XDG_CONFIG_HOME`, where
//!   git finds the person's own configuration and ignore file when it is not under `HOME`.
//! - A revision a person typed or an agent gave is refused when it is empty, starts with `-`
//!   or holds a control character, then resolved to an object id; only ids go further.
//! - What git writes is read as bytes, up to a limit per run; past it the run is killed.
//!   Standard error is kept up to [`MAX_STDERR`] for the message a failure gives.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};

/// How long one run of git may take before it is killed.
pub(super) const RUN_LIMIT: Duration = Duration::from_secs(10);
/// The most bytes of standard error kept for a failure's message.
pub(super) const MAX_STDERR: usize = 4 * 1024;
/// The most bytes a short answer (an id, a path) may take.
pub(super) const MAX_SHORT: usize = 4 * 1024;

/// The arguments every run starts with.
const PREFIX: [&str; 3] = ["--no-pager", "-c", "core.quotePath=false"];
/// The variables passed by name, besides the process module's allowlist.
const INHERIT: [&str; 1] = ["XDG_CONFIG_HOME"];

/// Why a run gave nothing to use.
#[derive(Debug)]
pub(crate) enum GitError {
    /// git could not be found or started.
    Spawn(io::Error),
    /// It ran past [`RUN_LIMIT`] and was killed.
    TimedOut,
    /// It wrote more than the run allows, and was killed.
    TooLong,
    /// It failed: its exit status and the start of what it wrote to standard error.
    Failed { status: Option<i32>, stderr: String },
    /// Its answer was not what was asked for.
    Unexpected(&'static str),
    /// A revision refused before git saw it.
    BadRevision,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "cannot run git: {e}"),
            Self::TimedOut => write!(f, "git took more than {} s", RUN_LIMIT.as_secs()),
            Self::TooLong => f.write_str("git wrote more than a review reads"),
            Self::Failed { status, stderr } => match status {
                Some(code) => write!(f, "git failed ({code}): {}", stderr.trim_end()),
                None => write!(f, "git was stopped: {}", stderr.trim_end()),
            },
            Self::Unexpected(what) => write!(f, "git gave an unexpected answer: {what}"),
            Self::BadRevision => f.write_str(
                "not a revision: it is empty, starts with '-' or holds a control character",
            ),
        }
    }
}

/// What a run wrote: its standard output, whether that was cut at the limit, its status and
/// the start of its standard error.
pub(super) struct Output {
    pub(super) stdout: Vec<u8>,
    pub(super) cut: bool,
    pub(super) status: ExitStatus,
    pub(super) stderr: String,
}

/// git, found once, run in one work tree.
#[derive(Debug)]
pub(crate) struct Git {
    executable: Executable,
    /// The top of the work tree, where every run starts.
    top: PathBuf,
}

impl Git {
    /// git from `PATH`, leaving out `project` and what is under it, run in `top`.
    pub(super) fn new(top: PathBuf, project: &Path) -> Result<Git, GitError> {
        let executable = Executable::from_path("git", Some(project)).map_err(GitError::Spawn)?;
        Ok(Git { executable, top })
    }

    /// The same git, run in `top` instead.
    pub(super) fn moved(&self, top: PathBuf) -> Git {
        Git {
            executable: self.executable.clone(),
            top,
        }
    }

    /// The top of the work tree.
    pub(super) fn top(&self) -> &Path {
        &self.top
    }

    /// Starts git with `args` after oso's own, `set` over the environment, and standard
    /// input as `stdin`.
    pub(super) fn start(
        &self,
        args: &[&OsStr],
        set: &[(&str, &OsStr)],
        stdin: Source,
    ) -> Result<Child, GitError> {
        let args: Vec<OsString> = PREFIX
            .iter()
            .map(OsString::from)
            .chain(args.iter().map(|arg| (*arg).to_owned()))
            .collect();
        let mut variables: Vec<(&str, &OsStr)> = vec![
            ("GIT_OPTIONAL_LOCKS", OsStr::new("0")),
            ("GIT_TERMINAL_PROMPT", OsStr::new("0")),
        ];
        variables.extend_from_slice(set);
        spawn(&Spawn {
            executable: &self.executable,
            args: &args,
            cwd: Some(&self.top),
            project: Some(&self.top),
            inherit: &INHERIT,
            stdin,
            stdout: Sink::Piped,
            stderr: Sink::Piped,
            own_process_group: false,
            set: &variables,
            terminal: None,
        })
        .map_err(GitError::Spawn)
    }

    /// Runs git with `args` and `set`, keeping at most `limit` bytes of its output. Past the
    /// limit it is killed, and the output says it was cut.
    pub(super) fn output(
        &self,
        args: &[&OsStr],
        set: &[(&str, &OsStr)],
        limit: usize,
    ) -> Result<Output, GitError> {
        let mut child = self.start(args, set, Source::Closed)?;
        let (Some(mut stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitError::Unexpected("no output streams"));
        };
        let child = Arc::new(Mutex::new(child));
        let (read, killed) = watched(&child, RUN_LIMIT, || {
            let stderr = thread::scope(|scope| {
                let stderr = scope.spawn(move || keep_start(stderr, MAX_STDERR));
                let read = read_at_most(&mut stdout, limit);
                if matches!(read, Ok((_, true))) {
                    kill(&child);
                }
                (read, stderr.join().unwrap_or_default())
            });
            (stderr.0, stderr.1)
        });
        let (read, stderr) = read;
        let status = lock(&child).wait().map_err(GitError::Spawn)?;
        if killed {
            return Err(GitError::TimedOut);
        }
        let (stdout, cut) = read.map_err(GitError::Spawn)?;
        Ok(Output {
            stdout,
            cut,
            status,
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }

    /// Runs git and gives its output if it succeeded and fit in `limit`.
    pub(super) fn run(
        &self,
        args: &[&OsStr],
        set: &[(&str, &OsStr)],
        limit: usize,
    ) -> Result<Vec<u8>, GitError> {
        let output = self.output(args, set, limit)?;
        if output.cut {
            return Err(GitError::TooLong);
        }
        if !output.status.success() {
            return Err(GitError::Failed {
                status: output.status.code(),
                stderr: output.stderr,
            });
        }
        Ok(output.stdout)
    }

    /// Resolves `revision`, as a person or an agent gave it, to the id of the object `peel`
    /// names (`commit` or `tree`), or `None` if there is no such object.
    pub(super) fn resolve(&self, revision: &str, peel: &str) -> Result<Option<Id>, GitError> {
        check_revision(revision)?;
        let spec = OsString::from(format!("{revision}^{{{peel}}}"));
        let args = [
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--quiet"),
            OsStr::new("--end-of-options"),
            &spec,
        ];
        let output = self.output(&args, &[], MAX_SHORT)?;
        if !output.status.success() {
            return Ok(None);
        }
        Id::parse(trim_line(&output.stdout))
            .map(Some)
            .ok_or(GitError::Unexpected("not an object id"))
    }

    /// The id of the empty tree, in this repository's hash.
    pub(super) fn empty_tree(&self) -> Result<Id, GitError> {
        let args = [
            OsStr::new("hash-object"),
            OsStr::new("-t"),
            OsStr::new("tree"),
            OsStr::new("--stdin"),
        ];
        let out = self.run(&args, &[], MAX_SHORT)?;
        Id::parse(trim_line(&out)).ok_or(GitError::Unexpected("not an object id"))
    }
}

/// An object id, as hexadecimal digits: 40 for SHA-1, 64 for SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Id(Box<str>);

impl Id {
    /// The id `bytes` spell, if they are one.
    pub(crate) fn parse(bytes: &[u8]) -> Option<Id> {
        let hex = matches!(bytes.len(), 40 | 64)
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
        hex.then(|| Id(String::from_utf8_lossy(bytes).into()))
    }

    /// Whether it is all zeros, as git writes for no object.
    pub(crate) fn is_zero(&self) -> bool {
        self.0.bytes().all(|byte| byte == b'0')
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn as_os_str(&self) -> &OsStr {
        OsStr::new(&*self.0)
    }
}

/// Refuses a revision git could read as an option or that holds a control character.
pub(super) fn check_revision(revision: &str) -> Result<(), GitError> {
    if revision.is_empty() || revision.starts_with('-') || revision.chars().any(char::is_control) {
        return Err(GitError::BadRevision);
    }
    Ok(())
}

/// `bytes` without the line break that ends them.
pub(super) fn trim_line(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

/// Runs `work` while a watcher kills `child` once `limit` has passed. It says whether the
/// watcher killed it.
pub(super) fn watched<T>(
    child: &Arc<Mutex<Child>>,
    limit: Duration,
    work: impl FnOnce() -> T,
) -> (T, bool) {
    let (done, finished) = mpsc::channel::<()>();
    thread::scope(|scope| {
        let watcher = scope.spawn(move || {
            let timed_out = matches!(
                finished.recv_timeout(limit),
                Err(mpsc::RecvTimeoutError::Timeout)
            );
            if timed_out {
                kill(child);
            }
            timed_out
        });
        let result = work();
        drop(done);
        (result, watcher.join().unwrap_or(false))
    })
}

/// Kills `child`; one that has already gone is no error.
pub(super) fn kill(child: &Mutex<Child>) {
    let _ = lock(child).kill();
}

pub(super) fn lock(child: &Mutex<Child>) -> std::sync::MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reads `from` to its end, keeping at most `limit` bytes. `true` if there was more.
fn read_at_most(from: &mut impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    from.take(cap).read_to_end(&mut kept)?;
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

#[cfg(test)]
mod tests {
    use super::{Id, check_revision, read_at_most, trim_line};

    #[test]
    fn a_revision_that_reads_as_an_option_or_holds_a_control_character_is_refused() {
        for bad in ["", "-x", "--output=/tmp/x", "main\n", "a\u{1b}[31m", "x\0y"] {
            assert!(check_revision(bad).is_err(), "{bad:?}");
        }
        for good in ["main", "HEAD~2", "v1.0^{commit}", "origin/feature-x"] {
            assert!(check_revision(good).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn only_full_lowercase_hex_ids_are_ids() {
        assert!(Id::parse(&[b'a'; 40]).is_some());
        assert!(Id::parse(&[b'0'; 64]).is_some_and(|id| id.is_zero()));
        for bad in [&b"abc"[..], &[b'A'; 40], &[b'g'; 40], &[b'a'; 41]] {
            assert!(Id::parse(bad).is_none(), "{bad:?}");
        }
        assert_eq!(trim_line(b"x\r\n"), b"x");
    }

    #[test]
    fn a_read_keeps_its_limit_and_says_there_was_more() {
        let (kept, cut) = read_at_most(&mut &b"abcdef"[..], 4).unwrap();
        assert_eq!((kept.as_slice(), cut), (&b"abcd"[..], true));
        let (kept, cut) = read_at_most(&mut &b"abcd"[..], 4).unwrap();
        assert_eq!((kept.as_slice(), cut), (&b"abcd"[..], false));
    }
}
