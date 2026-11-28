//! What an app launch on macOS takes from the person's login shell (ADR 0187).
//!
//! Must not depend on: `app`, `config`, `channel` or `sessions`: nothing from the
//! configuration, a project or an agent reaches the shell, whose command is fixed here.
//!
//! LaunchServices starts `oso.app` (from the Dock, Spotlight, Finder or `open -a`) with
//! launchd's environment, whose `PATH` is `/usr/bin:/bin:/usr/sbin:/sbin`, and in `/`. The
//! bundle's `LSEnvironment` sets [`MARKER`]; without it this module does nothing. With it,
//! before any thread but its reader, the account's login shell (the user database's, never
//! launchd's `SHELL`, else `/bin/bash`) runs once, bounded, and prints its environment, which
//! is merged into oso's own: its `PATH` replaces launchd's, other names come only where oso
//! has none, and the loader's and the shell's own are dropped. The marker is taken out of oso's
//! environment, so no child sees it; `oso-restart` sets it again for the instance it starts.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The variable `oso.app`'s `LSEnvironment` sets for every LaunchServices start.
pub(crate) const MARKER: &str = "OSO_LAUNCHED_AS_APP";

/// What the shell prints before its environment, so that what its profile printed first is
/// told from it.
const START: &[u8] = b"\0oso-env\0";

/// The shell's command: the line above, then the environment, each `NAME=VALUE` ended by NUL.
/// Fixed: nothing from outside oso is ever part of it.
const COMMAND: &str = "printf '\\000oso-env\\000'; exec /usr/bin/env -0";

/// How long the shell may take, its profile's work included.
const TIMEOUT: Duration = Duration::from_secs(2);

/// The most the shell may print.
const MAX_OUTPUT: usize = 1024 * 1024;

/// The shell when the account has none that runs.
const FALLBACK_SHELL: &str = "/bin/bash";

/// `LANG` when neither launchd nor the shell set it, so that a terminal's programs see UTF-8.
const FALLBACK_LANG: &str = "en_US.UTF-8";

/// What an app launch took.
#[derive(Debug)]
pub(crate) struct Taken {
    /// What `*Messages*` says: why the shell's environment was not taken, if it was not.
    pub(crate) note: Option<String>,
    /// How long it took.
    pub(crate) took: Duration,
}

/// Why the shell's environment was not taken.
#[derive(Debug)]
enum Failure {
    /// Neither the account's shell nor [`FALLBACK_SHELL`] is an executable.
    NoShell,
    Spawn(io::Error),
    Timeout(Duration),
    /// It printed no [`START`]: it failed before the command, or the command did.
    NoMarker,
    TooLong,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoShell => write!(f, "neither the account's shell nor {FALLBACK_SHELL} runs"),
            Self::Spawn(e) => write!(f, "the login shell did not start: {e}"),
            Self::Timeout(bound) => {
                write!(f, "the login shell took over {} s", bound.as_secs_f32())
            }
            Self::NoMarker => write!(f, "the login shell did not print its environment"),
            Self::TooLong => write!(f, "the login shell printed over {} KiB", MAX_OUTPUT / 1024),
        }
    }
}

/// With [`MARKER`] set, takes the login shell's environment into this process's, changes the
/// directory from `/` to the home, and says how it went; without it, does nothing. Waits up to
/// [`TIMEOUT`] on the shell. Must run before any other thread starts, as it sets variables.
/// When the shell fails, launchd's environment stays, but for the marker and `LANG`.
pub(crate) fn take_login_environment_if_app() -> Option<Taken> {
    std::env::var_os(MARKER)?;
    let started = Instant::now();
    let taken = shell(oso_platform::process::account_shell())
        .ok_or(Failure::NoShell)
        .and_then(|shell| {
            let args = [OsStr::new("-l"), OsStr::new("-c"), OsStr::new(COMMAND)];
            run(&shell, &args, TIMEOUT)
        })
        .and_then(|output| parse(&output));
    let (taken, note) = match taken {
        Ok(taken) => (taken, None),
        Err(failure) => (
            Vec::new(),
            Some(format!(
                "oso.app: {failure}; launchd's environment is kept (ADR 0187)"
            )),
        ),
    };
    let current: Vec<_> = std::env::vars_os().collect();
    let merged = merge(&current, taken);
    for (name, _) in &current {
        if !merged.iter().any(|(kept, _)| kept == name) {
            // SAFETY: no thread but `run`'s readers exists yet (`main` calls this before any
            // other), and a reader only reads its pipe and sends what it read: none reads or
            // writes the environment.
            unsafe { std::env::remove_var(name) };
        }
    }
    for (name, value) in &merged {
        if std::env::var_os(name).as_deref() != Some(value.as_os_str()) {
            // SAFETY: as above.
            unsafe { std::env::set_var(name, value) };
        }
    }
    home_if_at_root();
    Some(Taken {
        note,
        took: started.elapsed(),
    })
}

/// Changes this process's directory to the home when it is `/`, as LaunchServices starts it,
/// so that what is opened by a relative name and a terminal's shell start there.
fn home_if_at_root() {
    if std::env::current_dir().is_ok_and(|dir| dir == Path::new("/"))
        && let Some(home) = std::env::var_os("HOME").filter(|home| Path::new(home).is_absolute())
        && let Err(e) = std::env::set_current_dir(&home)
    {
        log::warn!("oso.app: cannot change to the home directory: {e}");
    }
}

/// The account's shell if it is an executable, else [`FALLBACK_SHELL`] if that is one.
fn shell(account: Option<PathBuf>) -> Option<PathBuf> {
    let runs = |path: &PathBuf| oso_platform::process::Executable::absolute(path.clone()).is_ok();
    account
        .filter(runs)
        .or_else(|| Some(PathBuf::from(FALLBACK_SHELL)).filter(runs))
}

/// Runs `shell` with `args`, in a process group of its own, and gives what it printed: at most
/// [`MAX_OUTPUT`] bytes and one more, so that [`parse`] can tell it was too long. Past
/// `timeout`, ends the group. How the shell ended does not matter.
fn run(shell: &Path, args: &[&OsStr], timeout: Duration) -> Result<Vec<u8>, Failure> {
    use oso_platform::process::{Executable, Signal, Sink, Source, Spawn, spawn};

    let deadline = Instant::now() + timeout;
    let executable = Executable::absolute(shell).map_err(Failure::Spawn)?;
    let args: Vec<OsString> = args.iter().map(|&arg| arg.to_owned()).collect();
    let mut child = spawn(&Spawn {
        executable: &executable,
        args: &args,
        cwd: None,
        project: None,
        inherit: &[],
        stdin: Source::Closed,
        stdout: Sink::Piped,
        stderr: Sink::Null,
        own_process_group: true,
        own_session: false,
        set: &[],
        terminal: None,
        pass_fd: None,
        args_logged: true,
    })
    .map_err(Failure::Spawn)?;
    let stdout = child.stdout.take();
    let (sent, read) = std::sync::mpsc::channel();
    // Not joined: what the shell left running could hold the pipe after the group ends. It
    // never touches the environment.
    let reader = std::thread::Builder::new()
        .name("oso-login-shell".into())
        .spawn(move || {
            let mut output = Vec::new();
            if let Some(stdout) = stdout {
                let cap = u64::try_from(MAX_OUTPUT + 1).unwrap_or(u64::MAX);
                let _ = stdout.take(cap).read_to_end(&mut output);
            }
            let _ = sent.send(output);
        });
    if let Err(e) = reader {
        end(&mut child);
        return Err(Failure::Spawn(e));
    }
    let output = read.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    // Its standard output ended, or was cut: the shell is waited for until the deadline too.
    while output.is_ok() && Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return output.map_err(|_| Failure::Timeout(timeout)),
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break,
        }
    }
    let _ = child.signal_group(Signal::Kill);
    end(&mut child);
    Err(Failure::Timeout(timeout))
}

/// Ends the shell and waits for it, so that it is not left unreaped.
fn end(child: &mut oso_platform::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The variables the shell printed after [`START`], in order, the first of each name: those
/// with a portable name (letters, digits and `_`, not first a digit) that are not the
/// loader's (`DYLD_*`, `LD_*`), the shell's own (`PWD`, `OLDPWD`, `SHLVL`, `_`) or [`MARKER`].
fn parse(output: &[u8]) -> Result<Vec<(OsString, OsString)>, Failure> {
    if output.len() > MAX_OUTPUT {
        return Err(Failure::TooLong);
    }
    // The last: `env` never prints it, as every line it prints has a `=`.
    let at = output
        .windows(START.len())
        .rposition(|window| window == START)
        .ok_or(Failure::NoMarker)?;
    let mut taken: Vec<(OsString, OsString)> = Vec::new();
    for entry in output[at + START.len()..].split(|&byte| byte == 0) {
        let Some(equals) = entry.iter().position(|&byte| byte == b'=') else {
            continue;
        };
        let (name, value) = (&entry[..equals], &entry[equals + 1..]);
        let Ok(name) = std::str::from_utf8(name) else {
            continue;
        };
        if !taken_name(name) || taken.iter().any(|(seen, _)| seen == name) {
            continue;
        }
        if let Some(value) = os_string(value) {
            taken.push((name.into(), value));
        }
    }
    Ok(taken)
}

/// Whether a variable called `name` may be taken from the shell.
fn taken_name(name: &str) -> bool {
    let mut chars = name.chars();
    let portable = chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    portable
        && !name.starts_with("DYLD_")
        && !name.starts_with("LD_")
        && !matches!(name, "PWD" | "OLDPWD" | "SHLVL" | "_" | MARKER)
}

/// A value as the system has it: any bytes on Unix, UTF-8 elsewhere.
fn os_string(bytes: &[u8]) -> Option<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(OsStr::from_bytes(bytes).to_owned())
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes).ok().map(OsString::from)
    }
}

/// The environment `current` becomes with `taken`: `taken`'s `PATH` if not empty, its other
/// names only where `current` has none, [`MARKER`] gone and `LANG` set if neither had it.
fn merge(
    current: &[(OsString, OsString)],
    taken: Vec<(OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    let mut merged: Vec<(OsString, OsString)> = current
        .iter()
        .filter(|(name, _)| name != MARKER)
        .cloned()
        .collect();
    for (name, value) in taken {
        match merged.iter_mut().find(|(have, _)| *have == name) {
            Some((_, have)) if name == "PATH" && !value.is_empty() => *have = value,
            Some(_) => {}
            None if name == "PATH" && value.is_empty() => {}
            None => merged.push((name, value)),
        }
    }
    if !merged.iter().any(|(name, _)| name == "LANG") {
        merged.push(("LANG".into(), FALLBACK_LANG.into()));
    }
    merged
}

#[cfg(test)]
mod tests;
