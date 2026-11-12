//! Running `git` for a review (ADR 0122): oso's own arguments and environment, every read
//! bounded, and a run past [`RUN_LIMIT`] killed.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs on the review's
//! thread and waits for git.
//!
//! - Every run starts `git --no-pager -c core.quotePath=false -c core.fsmonitor=false
//!   -c core.hooksPath=HOOKS`, where `HOOKS` is an empty private directory, so that no hook
//!   and no file system monitor the repository names runs; git hands these to the git it
//!   starts in an embedded repository too. Clean filters still run, as `git add` and a diff of
//!   the work tree need them. Each run is in the work tree's top, and gets the process
//!   module's allowlist plus `GIT_OPTIONAL_LOCKS=0` (a read takes no lock
//!   from the agent's own git), `GIT_TERMINAL_PROMPT=0`, `GIT_NO_LAZY_FETCH=1` (a partial
//!   clone fetches nothing), `GIT_NO_REPLACE_OBJECTS=1` (an object is what its id names),
//!   `GIT_CEILING_DIRECTORIES` set to the project's parent (git never looks above the trusted
//!   root for a repository) and, by name, `XDG_CONFIG_HOME`, where git finds the person's own
//!   configuration and ignore file when it is not under `HOME`. A parent whose path holds the
//!   list separator cannot be a ceiling, so such a root is refused before git runs.
//! - On Windows, a path handed to git loses its verbatim `\\?\` prefix, which git cannot take.
//! - A revision a person typed or an agent gave is refused when it is empty, starts with `-`
//!   or holds a control character, then resolved to an object id; only ids go further.
//! - What git writes is read as bytes, up to a limit per run; past it the run is killed.
//!   Standard error is kept up to [`MAX_STDERR`] for the message a failure gives.
//! - Its output is read on threads of their own, and a run past [`RUN_LIMIT`] is killed and
//!   left: a process git started may still hold its output open, and the review does not wait
//!   for it.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, ExitStatus};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};

use super::id::{Id, trim_line};

/// How long one run of git may take before it is killed.
pub(super) const RUN_LIMIT: Duration = Duration::from_secs(10);
/// The most bytes of standard error kept for a failure's message.
pub(super) const MAX_STDERR: usize = 4 * 1024;
/// The most bytes a short answer (an id, a path) may take.
pub(super) const MAX_SHORT: usize = 4 * 1024;

/// The arguments every run starts with, before the hooks directory.
const PREFIX: [&str; 5] = [
    "--no-pager",
    "-c",
    "core.quotePath=false",
    "-c",
    "core.fsmonitor=false",
];
/// What separates the directories of `GIT_CEILING_DIRECTORIES`.
const LIST_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };
/// The variables passed by name, besides the process module's allowlist.
const INHERIT: [&str; 1] = ["XDG_CONFIG_HOME"];
/// How often a run whose output has ended is asked whether it has exited.
const EXIT_POLL: Duration = Duration::from_millis(1);

/// Why a run gave nothing to use.
#[derive(Debug)]
pub(crate) enum GitError {
    /// git could not be found or started.
    Spawn(io::Error),
    /// Its output could not be read or its input written.
    Io(io::Error),
    /// It ran past [`RUN_LIMIT`] and was killed.
    TimedOut,
    /// It wrote more than the run allows, and was killed.
    TooLong,
    /// git could not be kept from looking above the project: the parent's path holds the
    /// list separator.
    NoCeiling,
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
            Self::Io(e) => write!(f, "cannot talk to git: {e}"),
            Self::TimedOut => write!(f, "git took more than {} s", RUN_LIMIT.as_secs()),
            Self::TooLong => f.write_str("git wrote more than a review reads"),
            Self::NoCeiling => write!(
                f,
                "git cannot be kept from looking above the project, as its parent's path holds \
                 a '{LIST_SEPARATOR}'"
            ),
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
#[derive(Clone, Debug)]
pub(crate) struct Git {
    executable: Executable,
    /// The top of the work tree, where every run starts.
    top: PathBuf,
    /// Where git stops looking for a repository: the project's parent.
    ceiling: OsString,
    /// `core.hooksPath=` an empty directory.
    hooks: OsString,
}

impl Git {
    /// git from `PATH`, leaving out `project` and what is under it, run in `project`, never
    /// finding a repository above it, and looking for hooks only in `hooks`, an empty private
    /// directory.
    pub(super) fn new(project: &Path, hooks: &Path) -> Result<Git, GitError> {
        let ceiling = project
            .parent()
            .map_or_else(OsString::new, |parent| for_git(parent).into_os_string());
        if ceiling.to_string_lossy().contains(LIST_SEPARATOR) {
            return Err(GitError::NoCeiling);
        }
        let executable = Executable::from_path("git", Some(project)).map_err(GitError::Spawn)?;
        let mut hooks_path = OsString::from("core.hooksPath=");
        hooks_path.push(for_git(hooks));
        Ok(Git {
            executable,
            top: for_git(project),
            ceiling,
            hooks: hooks_path,
        })
    }

    /// The same git, run in `top` instead.
    pub(super) fn in_top(&self, top: &Path) -> Git {
        Git {
            top: for_git(top),
            ..self.clone()
        }
    }

    /// The top of the work tree.
    pub(super) fn top(&self) -> &Path {
        &self.top
    }

    /// Starts git with `args` after oso's own, `set` over the environment, standard input as
    /// `stdin` and standard error as `stderr`. A path in `set` must have gone through
    /// [`for_git`].
    pub(super) fn start(
        &self,
        args: &[&OsStr],
        set: &[(&str, &OsStr)],
        stdin: Source,
        stderr: Sink,
    ) -> Result<Child, GitError> {
        let args: Vec<OsString> = PREFIX
            .iter()
            .map(OsString::from)
            .chain([OsString::from("-c"), self.hooks.clone()])
            .chain(args.iter().map(|arg| (*arg).to_owned()))
            .collect();
        let mut variables: Vec<(&str, &OsStr)> = vec![
            ("GIT_OPTIONAL_LOCKS", OsStr::new("0")),
            ("GIT_TERMINAL_PROMPT", OsStr::new("0")),
            ("GIT_NO_LAZY_FETCH", OsStr::new("1")),
            ("GIT_NO_REPLACE_OBJECTS", OsStr::new("1")),
        ];
        if !self.ceiling.is_empty() {
            variables.push(("GIT_CEILING_DIRECTORIES", &self.ceiling));
        }
        variables.extend_from_slice(set);
        spawn(&Spawn {
            executable: &self.executable,
            args: &args,
            cwd: Some(&self.top),
            project: Some(&self.top),
            inherit: &INHERIT,
            stdin,
            stdout: Sink::Piped,
            stderr,
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
        self.output_with(args, set, None, move |stdout| read_at_most(stdout, limit))
    }

    /// Runs git with `args` and `set`, writing `input` to its standard input if there is one,
    /// and keeping what `read` keeps of its output, which says whether it cut it. A cut run is
    /// killed.
    pub(super) fn output_with(
        &self,
        args: &[&OsStr],
        set: &[(&str, &OsStr)],
        input: Option<Vec<u8>>,
        read: impl FnOnce(ChildStdout) -> io::Result<(Vec<u8>, bool)> + Send + 'static,
    ) -> Result<Output, GitError> {
        let stdin = if input.is_some() {
            Source::Piped
        } else {
            Source::Closed
        };
        let mut child = self.start(args, set, stdin, Sink::Piped)?;
        let deadline = Instant::now() + RUN_LIMIT;
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
        let args = ["hash-object", "-t", "tree", "--stdin"].map(OsStr::new);
        let out = self.run(&args, &[], MAX_SHORT)?;
        Id::parse(trim_line(&out)).ok_or(GitError::Unexpected("not an object id"))
    }
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
        .name("oso-review-read".into())
        .spawn(move || {
            let _ = sent.send(read());
        })
        .map(drop)
}

/// Writes `input` to `child`'s standard input on a thread of its own, and closes it after: a
/// run that is given up on leaves the thread to end when the pipe does.
fn write_away(child: &mut Child, input: Vec<u8>) -> Result<(), GitError> {
    let mut writer = child
        .stdin
        .take()
        .ok_or(GitError::Unexpected("no input stream"))?;
    thread::Builder::new()
        .name("oso-review-write".into())
        .spawn(move || {
            let _ = writer.write_all(&input);
        })
        .map(drop)
        .map_err(GitError::Io)
}

/// Kills `child` and reaps it; one that has gone already is no error.
pub(super) fn stop(child: &mut Child) {
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

/// `path` as git can take it: on Windows, without the verbatim `\\?\` prefix that
/// `canonicalize` gives, which git cannot open a lock beside; elsewhere as it is.
pub(super) fn for_git(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut components = path.components();
        let Some(Component::Prefix(prefix)) = components.next() else {
            return path.to_owned();
        };
        let mut plain = match prefix.kind() {
            Prefix::VerbatimDisk(letter) => PathBuf::from(format!("{}:\\", char::from(letter))),
            Prefix::VerbatimUNC(server, share) => {
                let mut unc = OsString::from(r"\\");
                unc.push(server);
                unc.push(r"\");
                unc.push(share);
                unc.push(r"\");
                PathBuf::from(unc)
            }
            _ => return path.to_owned(),
        };
        for component in components {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => plain.push(name),
                // A verbatim path means these literally; spelled plainly they would not.
                Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                    return path.to_owned();
                }
            }
        }
        plain
    }
    #[cfg(not(windows))]
    path.to_owned()
}

/// Refuses a revision git could read as an option or that holds a control character.
pub(super) fn check_revision(revision: &str) -> Result<(), GitError> {
    if revision.is_empty() || revision.starts_with('-') || revision.chars().any(char::is_control) {
        return Err(GitError::BadRevision);
    }
    Ok(())
}

/// Reads `from` to its end, keeping at most `limit` bytes. `true` if there was more.
pub(super) fn read_at_most(mut from: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
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

#[cfg(test)]
mod tests {
    use super::{check_revision, read_at_most};

    #[cfg(windows)]
    #[test]
    fn a_path_for_git_loses_its_verbatim_prefix() {
        use super::for_git;
        use std::path::Path;
        let cases = [
            (r"\\?\C:\Users\a\state\index-1", r"C:\Users\a\state\index-1"),
            (r"\\?\UNC\server\share\repo", r"\\server\share\repo"),
            (r"C:\plain\path", r"C:\plain\path"),
        ];
        for (given, plain) in cases {
            assert_eq!(for_git(Path::new(given)), Path::new(plain), "{given}");
        }
    }

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
    fn a_read_keeps_its_limit_and_says_there_was_more() {
        let (kept, cut) = read_at_most(&b"abcdef"[..], 4).unwrap();
        assert_eq!((kept.as_slice(), cut), (&b"abcd"[..], true));
        let (kept, cut) = read_at_most(&b"abcd"[..], 4).unwrap();
        assert_eq!((kept.as_slice(), cut), (&b"abcd"[..], false));
    }
}
