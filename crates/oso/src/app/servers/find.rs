//! Finding a language server's program (ADR 0138, Which servers, A server inside the project
//! and Windows), on the server's thread before each start, after the trust check: first the
//! programs inside the project the entry names, each judged by its metadata alone; then an
//! absolute path as it is, or a name on `PATH` without the project; then Cargo's or Go's `bin`
//! directory, found from oso's own environment and home, never from the project. On Windows
//! only an `.exe` starts.
//!
//! Must not depend on: the UI thread or the editor; this looks at the file system, reading no
//! file's contents.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use oso_platform::privacy::Writers;
use oso_platform::process::Executable;

use crate::config::{AlsoIn, InProject};

/// What an entry says of where its program is.
#[derive(Debug, Clone)]
pub(super) struct Where {
    pub(super) in_project: Vec<InProject>,
    /// The program and its arguments, after those inside the project.
    pub(super) command: Vec<String>,
    pub(super) also_in: AlsoIn,
}

/// The program `how` names for the project at `root`, with its arguments, or why there is
/// none, for a person. Each program inside the project passed over says why in `passed`.
pub(super) fn program(
    how: &Where,
    root: &Path,
    passed: &mut Vec<String>,
) -> Result<(Executable, Vec<OsString>), String> {
    for choice in &how.in_project {
        match in_project(choice, root) {
            Ok(executable) => {
                let args = choice.args.iter().map(OsString::from).collect();
                return Ok((executable, args));
            }
            Err(why) => passed.push(why),
        }
    }
    let Some((name, args)) = how.command.split_first() else {
        return Err("no command".into());
    };
    let found = if Path::new(name).is_absolute() {
        Executable::absolute(name).map_err(|error| error.to_string())
    } else {
        Executable::from_path(name, Some(root)).or_else(|_| also_in(name, how.also_in, root))
    }?;
    only_exe(&found)?;
    Ok((found, args.iter().map(OsString::from).collect()))
}

/// `name` in the directory `also` names, or that it is not installed, saying where it looked.
fn also_in(name: &str, also: AlsoIn, root: &Path) -> Result<Executable, String> {
    let dir = match also {
        AlsoIn::Nowhere => None,
        AlsoIn::CargoBin => cargo_bin(),
        AlsoIn::GoBin => go_bin(),
    };
    // Never the project's own, nor the current directory's, as `PATH` leaves them out: a
    // shell started there may set `GOBIN` or `CARGO_HOME` to one.
    let dir = dir.filter(|dir| {
        !inside(dir, root) && !env::current_dir().is_ok_and(|current| inside(dir, &current))
    });
    match dir {
        Some(dir) => Executable::absolute(dir.join(exe(name))).map_err(|_| {
            format!(
                "{name} is not installed (looked on PATH and in {})",
                dir.display()
            )
        }),
        None => Err(format!("{name} is not installed (looked on PATH)")),
    }
}

/// The program `choice` names in the project's `node_modules/.bin`, if it may run: the file
/// `choice` needs is there and passes the same checks, the link is followed, and where it leads
/// is a regular file under the root, owned by this user, that no one else can change, nor any
/// directory or link on the way. Never on Windows yet, where no owner or access list is read:
/// npm's programs there are `.cmd` files anyway (ADR 0138, Windows).
fn in_project(choice: &InProject, root: &Path) -> Result<Executable, String> {
    let named = format!("node_modules/.bin/{}", choice.program);
    let not = |why: &str| format!("the project's {named} is not used: {why}");
    if cfg!(not(unix)) {
        return Err(not(
            "a program inside the project is not run on this system yet",
        ));
    }
    let root = fs::canonicalize(root).map_err(|_| not("the root cannot be read"))?;
    if let Some(needs) = choice.needs {
        judge(&root, needs).map_err(|why| not(&format!("{needs}: {why}")))?;
    }
    let target = judge(&root, &named).map_err(|why| not(&why))?;
    let found = Executable::absolute(&target).map_err(|error| not(&error.to_string()))?;
    only_exe(&found).map_err(|why| not(&why))?;
    Ok(found)
}

/// Where `relative`, under the canonical `root`, leads, if it may be used: links followed, a
/// regular file under the root, this user's, that no one else can change, nor any directory or
/// link on the way to it (ADR 0108's check); what a check finds otherwise, for a person.
fn judge(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = root.join(relative);
    let target = fs::canonicalize(&path).map_err(|_| "it is not there".to_owned())?;
    if !target.starts_with(root) {
        return Err("it leads outside the project".into());
    }
    // Through the links, so that each directory on the way is judged.
    let judged =
        oso_platform::privacy::who_can_change(&path).map_err(|_| "it cannot be read".to_owned())?;
    if !judged.metadata.is_file() {
        return Err("it is not a regular file".into());
    }
    if !oso_platform::privacy::is_this_users(&judged.metadata) {
        return Err("another user owns it".into());
    }
    if judged.file != Writers::Nobody || judged.way != Writers::Nobody {
        return Err("others can change it, or the way to it".into());
    }
    Ok(target)
}

/// Cargo's `bin` directory: `$CARGO_HOME/bin`, else `~/.cargo/bin`, from oso's own
/// environment.
fn cargo_bin() -> Option<PathBuf> {
    if let Some(home) = env::var_os("CARGO_HOME").map(PathBuf::from)
        && home.is_absolute()
    {
        return Some(home.join("bin"));
    }
    oso_platform::privacy::home().map(|home| home.join(".cargo").join("bin"))
}

/// Go's `bin` directory: `$GOBIN`, else the first `$GOPATH` entry's `bin`, else `~/go/bin`,
/// from oso's own environment.
fn go_bin() -> Option<PathBuf> {
    go_bin_of(
        env::var_os("GOBIN"),
        env::var_os("GOPATH"),
        oso_platform::privacy::home(),
    )
}

fn go_bin_of(
    gobin: Option<OsString>,
    gopath: Option<OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(gobin) = gobin.map(PathBuf::from).filter(|dir| dir.is_absolute()) {
        return Some(gobin);
    }
    if let Some(first) = gopath
        .as_deref()
        .and_then(|paths| env::split_paths(paths).next())
        .filter(|dir| dir.is_absolute())
    {
        return Some(first.join("bin"));
    }
    home.map(|home| home.join("go").join("bin"))
}

/// Whether `dir` is `base` or under it, as both resolve.
fn inside(dir: &Path, base: &Path) -> bool {
    let resolved = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    resolved(dir).starts_with(resolved(base))
}

/// `name` as the file it is on this system.
fn exe(name: &str) -> String {
    if cfg!(windows) && !name.to_ascii_lowercase().ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// On Windows, refuses a program that is not an `.exe`: a `.bat` or `.cmd` runs through
/// `cmd.exe`, whose quoting has let arguments run commands (CVE-2024-24576).
fn only_exe(found: &Executable) -> Result<(), String> {
    if cfg!(windows) && !is_exe(found.path()) {
        return Err(format!(
            "{} is not an .exe: oso starts no .bat or .cmd as a server",
            found.path().display()
        ));
    }
    Ok(())
}

/// Whether `path` names an `.exe`, ignoring case.
fn is_exe(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

#[cfg(test)]
mod tests;
