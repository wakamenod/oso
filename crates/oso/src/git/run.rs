//! Running `git` for a review (ADR 0122), for worktree sessions (ADR 0121) and for a git status
//! and its commits (ADR 0127): oso's own arguments and environment, every read bounded, and a
//! run past [`RUN_LIMIT`] killed.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs on a review's
//! thread, `oso-review`, on the worktree jobs' threads, `oso-sessions-git`, on a worktree
//! removal's, `oso-sessions-remove`, on a status's,
//! `oso-vcs`, and on a repository's run thread, `oso-vcs-run`, and waits for git.
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
//! - Its output is read on threads of their own, `oso-git-read` and `oso-git-write` (`collect`),
//!   and a run past [`RUN_LIMIT`] is killed and left: a process git started may still hold its output
//!   open, and the caller does not wait for it. A write that must not be cut short runs with
//!   no limit instead, in a group of its own that can be stopped (`long`); a worktree's
//!   removal runs that way in a session of its own ([`Git::detached`]), and oso never stops
//!   it (ADR 0121, "Worktree removal, as fixed").
//! - A run may let the repository's own hooks run ([`Git::with_own_hooks`]), for a commit,
//!   with the variables a hook needs; a push, a pull or a fetch gets the proxies too, and runs
//!   with no controlling terminal ([`Git::for_remote`]).

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, ExitStatus};
use std::time::Duration;

use oso_platform::process::{Child, Executable, Sink, Source, Spawn, spawn};

use super::id::{Id, trim_line};

mod collect;

pub(super) use collect::write_away;
pub(crate) use collect::{collect, read_at_most, stop};

/// How long one run of git may take before it is killed.
pub(crate) const RUN_LIMIT: Duration = Duration::from_secs(10);
/// The most bytes of standard error kept for a failure's message.
pub(crate) const MAX_STDERR: usize = 4 * 1024;
/// The most bytes a short answer (an id, a path) may take.
pub(crate) const MAX_SHORT: usize = 4 * 1024;

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
/// What a hook may need besides, for the person's own agents: an SSH key's, and GnuPG's, as a
/// hook or a signature asks them (ADR 0127, step 1b).
const HOOK_INHERIT: [&str; 3] = ["SSH_AUTH_SOCK", "GNUPGHOME", "GPG_AGENT_INFO"];
/// What a push, a pull or a fetch may need besides: the proxies git and `curl` take, in both
/// cases (ADR 0127, step 2, Q5).
const REMOTE_INHERIT: [&str; 8] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];
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
    /// It was asked to stop, and did (ADR 0127, step 1b).
    Stopped,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "cannot run git: {e}"),
            Self::Io(e) => write!(f, "cannot talk to git: {e}"),
            Self::TimedOut => write!(f, "git took more than {} s", RUN_LIMIT.as_secs()),
            Self::TooLong => f.write_str("git wrote more than oso reads"),
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
            Self::Stopped => f.write_str("git was stopped"),
        }
    }
}

/// What a run wrote: its standard output, whether that was cut at the limit, its status and
/// the start of its standard error.
pub(crate) struct Output {
    pub(crate) stdout: Vec<u8>,
    pub(crate) cut: bool,
    pub(crate) status: ExitStatus,
    pub(crate) stderr: String,
}

/// git, found once, run in one work tree.
#[derive(Clone, Debug)]
pub(crate) struct Git {
    executable: Executable,
    /// The top of the work tree, where every run starts.
    top: PathBuf,
    /// Where git stops looking for a repository: the project's parent.
    ceiling: OsString,
    /// `core.hooksPath=` an empty directory, or `None` where the repository's own hooks run.
    hooks: Option<OsString>,
    /// The names `[git] pass_env` passes on to a run whose hooks run.
    pass: Vec<String>,
    /// `GIT_DIR` and `GIT_WORK_TREE` of a linked worktree whose claim was checked, so that git
    /// finds no repository of its own (ADR 0121).
    worktree: Option<(OsString, OsString)>,
    /// It talks to a remote: it gets the proxies, and a long run starts in a session of its
    /// own with no controlling terminal (ADR 0127, step 2).
    remote: bool,
    /// A long run starts in a session of its own with no controlling terminal, as a remote's
    /// does, without the proxies: for a worktree's removal, which nothing stops (ADR 0121).
    detached: bool,
    /// How long a bounded run may take: [`RUN_LIMIT`], or less in a test.
    limit: Duration,
}

impl Git {
    /// git from `PATH`, leaving out `project` and what is under it, run in `project`, never
    /// finding a repository above it, and looking for hooks only in `hooks`, an empty private
    /// directory.
    pub(crate) fn new(project: &Path, hooks: &Path) -> Result<Git, GitError> {
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
            hooks: Some(hooks_path),
            pass: Vec::new(),
            worktree: None,
            remote: false,
            detached: false,
            limit: RUN_LIMIT,
        })
    }

    /// The same git, but the repository's own hooks run, as the person's own git would run
    /// them: for a commit (ADR 0011, ADR 0127). Such a run gets the variables a hook or a
    /// signature may need, the SSH agent's and GnuPG's, and the names in `pass`, and none of
    /// the reads' own (`GIT_OPTIONAL_LOCKS`, `GIT_NO_LAZY_FETCH`, `GIT_NO_REPLACE_OBJECTS`),
    /// which the git a hook runs would take from it. `GIT_EDITOR=:` and `GIT_SEQUENCE_EDITOR=:`
    /// keep any editor from opening.
    pub(crate) fn with_own_hooks(&self, pass: &[String]) -> Git {
        Git {
            hooks: None,
            pass: pass.to_vec(),
            ..self.clone()
        }
    }

    /// The same git with its own hooks, for a push, a pull or a fetch: it gets the proxies too,
    /// and a long run starts in a session of its own with no controlling terminal, so that no
    /// `ssh` or credential helper it starts can ask on the terminal oso was started from, where
    /// git would wait unseen until stopped; one fails instead (ADR 0127, step 2, Q3, Q5).
    pub(crate) fn for_remote(&self, pass: &[String]) -> Git {
        Git {
            remote: true,
            ..self.with_own_hooks(pass)
        }
    }

    /// The same git, whose long runs start in a session of their own with no controlling
    /// terminal: what it starts that opens the terminal fails at once instead of stopping, and
    /// closing the terminal oso was started from sends it nothing.
    pub(crate) fn detached(&self) -> Git {
        Git {
            detached: true,
            ..self.clone()
        }
    }

    /// The same git, whose bounded runs are killed after `limit` instead of [`RUN_LIMIT`].
    #[cfg(all(test, unix))]
    pub(crate) fn with_limit(&self, limit: Duration) -> Git {
        Git {
            limit,
            ..self.clone()
        }
    }

    /// The same git, run in the linked worktree at `work_tree`, whose own directory in the main
    /// repository is `git_dir`: both are given to git, so that it looks for no repository. The
    /// caller has checked the worktree's claim and the main repository's record of it.
    pub(crate) fn for_worktree(&self, git_dir: &Path, work_tree: &Path) -> Git {
        Git {
            top: for_git(work_tree),
            worktree: Some((
                for_git(git_dir).into_os_string(),
                for_git(work_tree).into_os_string(),
            )),
            ..self.clone()
        }
    }

    /// The same git, run in `top` instead.
    pub(crate) fn in_top(&self, top: &Path) -> Git {
        Git {
            top: for_git(top),
            ..self.clone()
        }
    }

    /// The top of the work tree.
    pub(crate) fn top(&self) -> &Path {
        &self.top
    }

    /// Starts git with `args` after oso's own, `set` over the environment, standard input as
    /// `stdin` and standard error as `stderr`. A path in `set` must have gone through
    /// [`for_git`]. Its output is read only through [`collect`], which bounds it and kills a
    /// run past [`RUN_LIMIT`], or the child is given to [`stop`].
    pub(crate) fn start(
        &self,
        args: &[&OsStr],
        set: &[(&str, &OsStr)],
        stdin: Source,
        stderr: Sink,
    ) -> Result<Child, GitError> {
        self.spawn_with(args, set, stdin, stderr, false)
    }

    /// [`Git::start`], in a process group of its own if `group`, so that ending the group
    /// ends what git started too; for a remote, in a session of its own instead, which leads
    /// such a group.
    pub(super) fn spawn_with(
        &self,
        args: &[&OsStr],
        set: &[(&str, &OsStr)],
        stdin: Source,
        stderr: Sink,
        group: bool,
    ) -> Result<Child, GitError> {
        let hooks = self
            .hooks
            .iter()
            .flat_map(|hooks| [OsString::from("-c"), hooks.clone()]);
        let args: Vec<OsString> = PREFIX
            .iter()
            .map(OsString::from)
            .chain(hooks)
            .chain(args.iter().map(|arg| (*arg).to_owned()))
            .collect();
        let mut variables: Vec<(&str, &OsStr)> = if self.hooks.is_some() {
            vec![
                ("GIT_OPTIONAL_LOCKS", OsStr::new("0")),
                ("GIT_TERMINAL_PROMPT", OsStr::new("0")),
                ("GIT_NO_LAZY_FETCH", OsStr::new("1")),
                ("GIT_NO_REPLACE_OBJECTS", OsStr::new("1")),
            ]
        } else {
            vec![
                ("GIT_TERMINAL_PROMPT", OsStr::new("0")),
                ("GIT_EDITOR", OsStr::new(":")),
                // `sequence.editor` would open one for a todo list (ADR 0127, step 3a).
                ("GIT_SEQUENCE_EDITOR", OsStr::new(":")),
            ]
        };
        let mut inherit: Vec<&str> = INHERIT.to_vec();
        if self.hooks.is_none() {
            inherit.extend(HOOK_INHERIT);
            inherit.extend(self.pass.iter().map(String::as_str));
        }
        if self.remote {
            inherit.extend(REMOTE_INHERIT);
        }
        if !self.ceiling.is_empty() {
            variables.push(("GIT_CEILING_DIRECTORIES", &self.ceiling));
        }
        if let Some((git_dir, work_tree)) = &self.worktree {
            variables.push(("GIT_DIR", git_dir));
            variables.push(("GIT_WORK_TREE", work_tree));
        }
        variables.extend_from_slice(set);
        spawn(&Spawn {
            executable: &self.executable,
            args: &args,
            cwd: Some(&self.top),
            project: Some(&self.top),
            inherit: &inherit,
            stdin,
            stdout: Sink::Piped,
            stderr,
            own_process_group: group && !(self.remote || self.detached),
            own_session: group && (self.remote || self.detached),
            set: &variables,
            terminal: None,
            pass_fd: None,
            args_logged: true,
        })
        .map_err(GitError::Spawn)
    }

    /// Runs git with `args` and `set`, keeping at most `limit` bytes of its output. Past the
    /// limit it is killed, and the output says it was cut.
    pub(crate) fn output(
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
    pub(crate) fn output_with(
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
        let child = self.start(args, set, stdin, Sink::Piped)?;
        collect(child, input, read, self.limit)
    }

    /// Runs git and gives its output if it succeeded and fit in `limit`.
    pub(crate) fn run(
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
    pub(crate) fn resolve(&self, revision: &str, peel: &str) -> Result<Option<Id>, GitError> {
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
    pub(crate) fn empty_tree(&self) -> Result<Id, GitError> {
        let args = ["hash-object", "-t", "tree", "--stdin"].map(OsStr::new);
        let out = self.run(&args, &[], MAX_SHORT)?;
        Id::parse(trim_line(&out)).ok_or(GitError::Unexpected("not an object id"))
    }
}

/// `path` as git can take it: on Windows, without the verbatim `\\?\` prefix that
/// `canonicalize` gives, which git cannot open a lock beside; elsewhere as it is.
pub(crate) fn for_git(path: &Path) -> PathBuf {
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
pub(crate) fn check_revision(revision: &str) -> Result<(), GitError> {
    if revision.is_empty() || revision.starts_with('-') || revision.chars().any(char::is_control) {
        return Err(GitError::BadRevision);
    }
    Ok(())
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
