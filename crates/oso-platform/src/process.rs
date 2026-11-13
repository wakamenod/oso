//! Spawning child processes: the only place allowed to (ADR 0010, ADR 0047).
//!
//! Must not depend on: `control` or `text`, or any idea of what is being run;
//! callers decide that and name the extra variables a program needs.
//!
//! - An executable comes from an absolute path, or from `PATH` without the
//!   current directory, the project and any relative or missing entry, so a
//!   repository cannot put a program of its own on the search path.
//! - A child gets an environment built from an allowlist, plus variables the
//!   caller sets by value. Anything else, such as `LD_PRELOAD`, `DYLD_*` or an
//!   API token, stays behind.
//! - Standard input is closed, unless the caller pipes it ([`Source::Piped`]) or the child
//!   runs in a terminal ([`pty`]).
//! - A caller can ask for a process group of its own, so that Ctrl-C leaves it.
//! - Every spawn is logged, with the names of the variables but not their
//!   values.
//! - [`account_shell`] reads the account's login shell from the user database.
//!
//! In every build since the terminal ships (ADR 0120).

#[cfg(unix)]
mod account;
pub mod pty;

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Starts the log line of every spawn, so that a log can be searched for them.
const SPAWN_MARKER: &str = "oso-process-spawn";

/// The variables every child gets, if set. `PATH` is added separately.
const BASE_ENV: [&str; 12] = [
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "LANG",
    "TERM",
    // Windows cannot start a process without the first of these.
    "SYSTEMROOT",
    "USERPROFILE",
    "TEMP",
    "TMP",
    "PATHEXT",
    "COMSPEC",
];

/// An executable found by the rules above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executable(PathBuf);

impl Executable {
    /// The executable at `path`, which must be absolute.
    pub fn absolute(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not an absolute path", path.display()),
            ));
        }
        if !is_executable(&path) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not an executable file", path.display()),
            ));
        }
        Ok(Self(path))
    }

    /// The executable called `name` on `PATH`, leaving out the current
    /// directory, `project` and everything under it, and entries that are
    /// relative or do not exist.
    pub fn from_path(name: &str, project: Option<&Path>) -> io::Result<Self> {
        let not_found = || {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{name} is not on PATH, leaving out the current directory and the project"),
            )
        };
        if Path::new(name).file_name() != Some(OsStr::new(name)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} is not a plain program name"),
            ));
        }
        let path = env::var_os("PATH").ok_or_else(not_found)?;
        let current = env::current_dir().ok();
        search_dirs(&path, current.as_deref(), project)
            .iter()
            .find_map(|dir| find_in(dir, name))
            .map(Self)
            .ok_or_else(not_found)
    }

    /// Where the executable is.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Where a child's output stream goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sink {
    /// Goes where this process's goes.
    Inherit,
    /// Readable through the [`Child`].
    Piped,
    /// Thrown away.
    Null,
}

impl From<Sink> for Stdio {
    fn from(sink: Sink) -> Self {
        match sink {
            Sink::Inherit => Self::inherit(),
            Sink::Piped => Self::piped(),
            Sink::Null => Self::null(),
        }
    }
}

/// Where a child's standard input comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Nothing: the child reads the end of its input at once.
    Closed,
    /// Written through the [`Child`]'s `stdin`.
    Piped,
}

impl From<Source> for Stdio {
    fn from(source: Source) -> Self {
        match source {
            Source::Closed => Self::null(),
            Source::Piped => Self::piped(),
        }
    }
}

/// A process to start. Its `Debug` shows the names in `set`, never their values.
#[derive(Clone, Copy)]
pub struct Spawn<'a> {
    pub executable: &'a Executable,
    pub args: &'a [OsString],
    /// The directory to run in, this process's if `None`.
    pub cwd: Option<&'a Path>,
    /// The project the child works on. It and everything under it are left out
    /// of the child's `PATH`.
    pub project: Option<&'a Path>,
    /// Variables this program needs on top of the base set, by name. Refused
    /// as `set`'s names are.
    pub inherit: &'a [&'a str],
    /// Its standard input. It must be [`Source::Closed`] for a child in a terminal.
    pub stdin: Source,
    pub stdout: Sink,
    pub stderr: Sink,
    /// Starts it in a process group of its own, so that what a terminal signals to
    /// its foreground group, such as Ctrl-C, does not reach it. It then outlives
    /// this process. Only Unix has this: the one caller needs the control socket,
    /// which is not supported elsewhere (ADR 0032, ADR 0055).
    pub own_process_group: bool,
    /// Variables set to these values, after the allowlist and over it. Their
    /// values are not logged. A name that is empty, holds `=` or NUL, or is
    /// `PATH` (this module's own), `LD_*` or `DYLD_*` (the loader's) is refused.
    /// Names and values here and in `inherit` come from oso or the person's own
    /// configuration, never from a project's files, a terminal's output or the
    /// agent channel: a shell runs what some variables name.
    pub set: &'a [(&'a str, &'a OsStr)],
    /// The terminal the child runs in: its standard input, output and error,
    /// and its controlling terminal, in a session of its own. `stdin` must then
    /// be [`Source::Closed`], `stdout` and `stderr` [`Sink::Inherit`], and
    /// `own_process_group` false.
    /// Such a spawn forks this whole process, which can take milliseconds, so
    /// it never runs on the UI thread.
    pub terminal: Option<&'a pty::Slave>,
}

impl fmt::Debug for Spawn<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Spawn")
            .field("executable", self.executable)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("project", &self.project)
            .field("inherit", &self.inherit)
            .field("stdin", &self.stdin)
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .field("own_process_group", &self.own_process_group)
            .field(
                "set",
                &self.set.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .field("terminal", &self.terminal)
            .finish()
    }
}

/// Starts a process.
pub fn spawn(request: &Spawn<'_>) -> io::Result<Child> {
    check(request)?;
    let current = env::current_dir().ok();
    let path = env::var_os("PATH").unwrap_or_default();
    let dirs = search_dirs(&path, current.as_deref(), request.project);
    let path = env::join_paths(dirs).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let variables = allowed_env(env::vars_os(), request.inherit);
    log::info!(
        "{SPAWN_MARKER}: {} {:?} in {:?}{}{} with PATH and {:?}",
        request.executable.path().display(),
        request.args,
        request.cwd,
        if request.terminal.is_some() {
            " in a terminal"
        } else {
            ""
        },
        if request.stdin == Source::Piped {
            " with standard input piped"
        } else {
            ""
        },
        variables
            .iter()
            .map(|(name, _)| name.as_os_str())
            .chain(request.set.iter().map(|(name, _)| OsStr::new(name)))
            .collect::<Vec<_>>(),
    );
    #[expect(
        clippy::disallowed_methods,
        reason = "this module is the one place allowed to spawn (ADR 0010)"
    )]
    let mut command = Command::new(request.executable.path());
    command
        .args(request.args)
        .env_clear()
        .envs(variables)
        .env("PATH", path)
        .envs(request.set.iter().copied())
        .stdin(request.stdin)
        .stdout(request.stdout)
        .stderr(request.stderr);
    if let Some(cwd) = request.cwd {
        command.current_dir(cwd);
    }
    #[cfg(unix)]
    if request.own_process_group {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    if let Some(terminal) = request.terminal {
        pty::run_in(terminal, &mut command)?;
    }
    command.spawn()
}

/// Refuses what [`Spawn`] says it refuses.
fn check(request: &Spawn<'_>) -> io::Result<()> {
    let invalid = |message: String| Err(io::Error::new(io::ErrorKind::InvalidInput, message));
    if request.terminal.is_some()
        && (request.stdin != Source::Closed
            || request.stdout != Sink::Inherit
            || request.stderr != Sink::Inherit
            || request.own_process_group)
    {
        return invalid(
            "a child in a terminal has the terminal as its streams and its own session".into(),
        );
    }
    let names = request
        .set
        .iter()
        .map(|(name, _)| name)
        .chain(request.inherit);
    match names.into_iter().find(|name| !may_pass(name)) {
        Some(name) => invalid(format!("{name:?} may not be passed to a child")),
        None => Ok(()),
    }
}

/// Whether a caller may pass the variable `name`, by value or by name.
fn may_pass(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let name = if cfg!(windows) { upper.as_str() } else { name };
    !name.is_empty()
        && !name.contains(['=', '\0'])
        && name != "PATH"
        && !name.starts_with("LD_")
        && !name.starts_with("DYLD_")
}

/// The shell the user database gives the account oso runs as, if it is an
/// absolute path. `None` where there is no user database. It may wait on a
/// directory service, so it never runs on the UI thread.
pub fn account_shell() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        account::shell()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The directories of `path`, a `PATH` value, that may supply a program: only
/// absolute ones that exist, and neither `current` nor `project` or anything
/// under it. One that does not exist cannot be told apart from the project, and
/// could appear later.
fn search_dirs(path: &OsStr, current: Option<&Path>, project: Option<&Path>) -> Vec<PathBuf> {
    let real = |dir: &Path| fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned());
    let current = current.map(real);
    let project = project.map(real);
    env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .filter(|dir| {
            fs::canonicalize(dir).is_ok_and(|dir| {
                current.as_deref() != Some(&*dir)
                    && project
                        .as_deref()
                        .is_none_or(|project| !dir.starts_with(project))
            })
        })
        .collect()
}

/// The variables of `variables` that a child may see: the base set, `LC_*`
/// and the names in `inherit`.
fn allowed_env(
    variables: impl Iterator<Item = (OsString, OsString)>,
    inherit: &[&str],
) -> Vec<(OsString, OsString)> {
    variables
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            name.starts_with("LC_")
                || BASE_ENV.iter().chain(inherit).any(|allowed| {
                    if cfg!(windows) {
                        name.eq_ignore_ascii_case(allowed)
                    } else {
                        name == *allowed
                    }
                })
        })
        .collect()
}

/// The program called `name` in `dir`.
fn find_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    if is_executable(&path) {
        return Some(path);
    }
    let path = dir.join(format!("{name}.exe"));
    (cfg!(windows) && is_executable(&path)).then_some(path)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

#[cfg(test)]
mod tests {
    use super::{Executable, allowed_env, search_dirs};
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    /// A fresh directory for one test, with no links in its path.
    fn scratch_dir(name: &str) -> PathBuf {
        let name = format!("oso-process-test-{}-{name}", std::process::id());
        let path = fs::canonicalize(env::temp_dir()).unwrap().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        path
    }

    fn path_of(dirs: &[&Path]) -> OsString {
        env::join_paths(dirs).unwrap()
    }

    /// The process group of a long `sleep`, started with or without its own.
    #[cfg(unix)]
    fn group_of_a_child(own_process_group: bool) -> (u32, u32) {
        use super::{Sink, Source, Spawn, spawn};
        use std::io::Read;
        let start = |name: &str, args: &[OsString], stdout: Sink| {
            spawn(&Spawn {
                executable: &Executable::from_path(name, None).unwrap(),
                args,
                cwd: None,
                project: None,
                inherit: &[],
                stdin: Source::Closed,
                stdout,
                stderr: Sink::Null,
                own_process_group: name == "sleep" && own_process_group,
                set: &[],
                terminal: None,
            })
            .unwrap()
        };
        let mut child = start("sleep", &["60".into()], Sink::Null);
        let args = [
            "-o".into(),
            "pgid=".into(),
            "-p".into(),
            child.id().to_string().into(),
        ];
        let mut ps = start("ps", &args, Sink::Piped);
        let mut output = String::new();
        ps.stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        ps.wait().unwrap();
        let pid = child.id();
        child.kill().unwrap();
        child.wait().unwrap();
        (pid, output.trim().parse().unwrap())
    }

    #[cfg(unix)]
    #[test]
    fn a_child_has_its_own_process_group_only_if_asked() {
        let (pid, group) = group_of_a_child(true);
        assert_eq!(group, pid, "it leads a group of its own");
        let (pid, group) = group_of_a_child(false);
        assert_ne!(group, pid, "it stays in this process's group");
    }

    #[test]
    fn only_absolute_paths_are_accepted_as_absolute() {
        for relative in ["cargo", "./cargo", "bin/cargo", ""] {
            let error = Executable::absolute(relative).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{relative:?}");
        }
        let directory = scratch_dir("absolute");
        let error = Executable::absolute(&directory).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "a directory");
        let error = Executable::absolute(directory.join("nothing")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_loader_and_path_variables_may_not_be_passed() {
        use super::may_pass;
        for name in [
            "",
            "A=B",
            "A\0B",
            "PATH",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
        ] {
            assert!(!may_pass(name), "{name:?}");
        }
        for name in ["TERM", "OSO_TERMINAL_TOKEN", "SSH_AUTH_SOCK"] {
            assert!(may_pass(name), "{name:?}");
        }
        // Windows reads names in any case; the loaders and shells of Unix do not.
        for name in ["Path", "ld_preload"] {
            assert_eq!(may_pass(name), !cfg!(windows), "{name:?}");
        }
    }

    #[test]
    fn a_program_name_is_not_a_path() {
        for name in ["./cargo", "bin/cargo", "../cargo", ".", ".."] {
            let error = Executable::from_path(name, None).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{name:?}");
        }
    }

    #[test]
    fn the_search_leaves_out_relative_entries_the_current_directory_and_the_project() {
        let root = scratch_dir("search");
        let (current, project, nested, tools) = (
            root.join("current"),
            root.join("project"),
            root.join("project").join("bin"),
            root.join("tools"),
        );
        for dir in [&current, &project, &nested, &tools] {
            fs::create_dir_all(dir).unwrap();
        }
        let path = path_of(&[
            Path::new("relative"),
            Path::new("."),
            &current,
            &project,
            &nested,
            &tools,
        ]);
        assert_eq!(
            search_dirs(&path, Some(&current), Some(&project)),
            std::slice::from_ref(&tools)
        );
        // The same directory, reached another way.
        let around = path_of(&[&project.join("..").join("project"), &tools]);
        assert_eq!(
            search_dirs(&around, None, Some(&project)),
            std::slice::from_ref(&tools)
        );
        // What does not exist yet may never be searched.
        let missing = path_of(&[&root.join("missing"), &tools]);
        assert_eq!(
            search_dirs(&missing, None, None),
            std::slice::from_ref(&tools)
        );
        // An empty entry means the current directory to a shell.
        let empty = env::join_paths([Path::new(""), &tools]).unwrap();
        assert_eq!(search_dirs(&empty, None, None), [tools]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_environment_is_an_allowlist() {
        let variables = [
            ("HOME", "/h"),
            ("LC_ALL", "C"),
            ("RUST_LOG", "debug"),
            ("LD_PRELOAD", "/evil.so"),
            ("DYLD_INSERT_LIBRARIES", "/evil.dylib"),
            ("AWS_SECRET_ACCESS_KEY", "s3cret"),
            ("PATH", "/ignored"),
            ("home", "/lower"),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let names = |inherit: &[&str]| -> Vec<String> {
            allowed_env(variables.clone().into_iter(), inherit)
                .into_iter()
                .map(|(name, _)| name.into_string().unwrap())
                .collect()
        };
        let lower = cfg!(windows).then(|| "home".to_owned());
        let expected = |extra: &[&str]| -> Vec<String> {
            ["HOME", "LC_ALL"]
                .iter()
                .chain(extra)
                .map(ToString::to_string)
                .chain(lower.clone())
                .collect()
        };
        assert_eq!(names(&[]), expected(&[]));
        assert_eq!(names(&["RUST_LOG"]), expected(&["RUST_LOG"]));
    }

    #[cfg(unix)]
    #[test]
    fn a_child_runs_where_it_is_told_with_only_the_allowed_variables() {
        use super::{Sink, Source, Spawn, spawn};
        use std::io::Read;

        let program = Executable::from_path("sh", None).unwrap();
        let args = [OsString::from("-c"), OsString::from("pwd; env; cat")];
        let cwd = scratch_dir("child");
        let mut child = spawn(&Spawn {
            executable: &program,
            args: &args,
            cwd: Some(&cwd),
            project: None,
            inherit: &[],
            stdin: Source::Closed,
            stdout: Sink::Piped,
            stderr: Sink::Null,
            own_process_group: false,
            set: &[],
            terminal: None,
        })
        .unwrap();
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(child.wait().unwrap().success(), "cat sees a closed stdin");
        let mut lines = output.lines();
        assert_eq!(Path::new(lines.next().unwrap()), cwd);
        for line in lines {
            let name = line.split('=').next().unwrap();
            // The shell adds a few variables of its own.
            let known = [
                "PATH",
                "PWD",
                "OLDPWD",
                "SHLVL",
                "_",
                "__CF_USER_TEXT_ENCODING",
            ];
            assert!(
                known.contains(&name) || name.starts_with("LC_") || super::BASE_ENV.contains(&name),
                "{line}"
            );
        }
        fs::remove_dir_all(cwd).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_piped_standard_input_reaches_the_child() {
        use super::{Sink, Source, Spawn, spawn};
        use std::io::{Read, Write};

        let program = Executable::from_path("cat", None).unwrap();
        let mut child = spawn(&Spawn {
            executable: &program,
            args: &[],
            cwd: None,
            project: None,
            inherit: &[],
            stdin: Source::Piped,
            stdout: Sink::Piped,
            stderr: Sink::Null,
            own_process_group: false,
            set: &[],
            terminal: None,
        })
        .unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(b"one\ntwo\n").unwrap();
        drop(input);
        let mut output = Vec::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_end(&mut output)
            .unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(output, b"one\ntwo\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_run_is_not_an_executable() {
        let directory = scratch_dir("mode");
        let file = directory.join("not-a-program");
        fs::write(&file, b"x").unwrap();
        assert_eq!(
            Executable::absolute(&file).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
