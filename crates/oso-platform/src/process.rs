//! Spawning child processes: the only place allowed to (ADR 0010, ADR 0047).
//!
//! Must not depend on: `control` or `text`, or any idea of what is being run;
//! callers decide that and name the extra variables a program needs.
//!
//! - An executable comes from an absolute path, or from `PATH` without the
//!   current directory, the project and any relative or missing entry, so a
//!   repository cannot put a program of its own on the search path.
//! - A child gets an environment built from an allowlist. Anything else,
//!   such as `LD_PRELOAD`, `DYLD_*` or an API token, stays behind.
//! - Standard input is closed.
//! - Every spawn is logged, with the names of the variables but not their
//!   values.
//!
//! Behind the `dev` feature until a release feature needs it.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Appears in every binary that can spawn, and in no other.
/// `just release-safety` looks for it in release binaries.
const SPAWN_MARKER: &str = "oso-dev-process-spawn";

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

/// A process to start.
#[derive(Debug, Clone, Copy)]
pub struct Spawn<'a> {
    pub executable: &'a Executable,
    pub args: &'a [OsString],
    /// The directory to run in, this process's if `None`.
    pub cwd: Option<&'a Path>,
    /// The project the child works on. It and everything under it are left out
    /// of the child's `PATH`.
    pub project: Option<&'a Path>,
    /// Variables this program needs on top of the base set, by name.
    pub inherit: &'a [&'a str],
    pub stdout: Sink,
    pub stderr: Sink,
}

/// Starts a process.
pub fn spawn(request: &Spawn<'_>) -> io::Result<Child> {
    let current = env::current_dir().ok();
    let path = env::var_os("PATH").unwrap_or_default();
    let dirs = search_dirs(&path, current.as_deref(), request.project);
    let path = env::join_paths(dirs).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let variables = allowed_env(env::vars_os(), request.inherit);
    log::info!(
        "{SPAWN_MARKER}: {} {:?} in {:?} with PATH and {:?}",
        request.executable.path().display(),
        request.args,
        request.cwd,
        variables.iter().map(|(name, _)| name).collect::<Vec<_>>(),
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
        .stdin(Stdio::null())
        .stdout(request.stdout)
        .stderr(request.stderr);
    if let Some(cwd) = request.cwd {
        command.current_dir(cwd);
    }
    command.spawn()
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
        use super::{Sink, Spawn, spawn};
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
            stdout: Sink::Piped,
            stderr: Sink::Null,
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
