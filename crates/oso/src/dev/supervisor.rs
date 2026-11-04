//! `oso dev`: rebuild on source changes and restart without losing state
//! (ADR 0002, ADR 0023, ADR 0047). The editor itself never runs cargo.
//!
//! Must not depend on: `app`, winit or editor state; it runs without a window
//! and reaches instances only through their control sockets.
//!
//! It trusts the repository it runs in, since a build runs that repository's
//! code anyway. New dependencies are not fetched: the build is offline.
//!
//! On a change it runs an incremental build, shown in the terminal. If that
//! fails, the instance keeps running. If it succeeds, the new binary starts,
//! takes over the old instance's state, and only then does the old one exit.
//! A new binary that cannot start is stopped and the old one kept. One that
//! refuses the state stays open, clean, beside the old one until the next
//! build.
//!
//! The supervisor keeps running the code it was started with, so changes to
//! this file take effect on the next `just dev`.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::control::ControlDir;
use oso_platform::process::{Executable, Sink, Spawn, spawn};
use serde_json::Value;

use super::ctl;
use super::protocol;
use super::restart::{Handover, Instance, hand_over};
use super::watch::Sources;

const USAGE: &str = "\
usage: oso dev [FILE]

Runs oso, rebuilds it when a Rust source changes, and restarts it with the same
buffers, cursors and window. Run it from the repository root (`just dev`).";

/// How often to look at the sources and the instance.
const POLL: Duration = Duration::from_millis(100);

/// The most polls to wait for a burst of saves to end before building.
const MAX_SETTLE: u32 = 10;

/// How long a new instance may take to open its window.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait between looks at a starting instance.
const START_POLL: Duration = Duration::from_millis(20);

/// How long an instance may take to exit before it is killed.
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// What cargo may need beyond the base set. Dropping one that a build script
/// watches would rebuild dependencies, so these follow the user's shell.
const CARGO_ENV: [&str; 16] = [
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_JOBS",
    "CARGO_INCREMENTAL",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "RUSTFLAGS",
    "RUSTC",
    "RUSTC_WRAPPER",
    "CC",
    "CXX",
    "AR",
    "SDKROOT",
    "DEVELOPER_DIR",
    "MACOSX_DEPLOYMENT_TARGET",
    "PKG_CONFIG_PATH",
];

/// What an editor instance needs beyond the base set: its log filter, and how
/// to reach the display and the control directory.
const INSTANCE_ENV: [&str; 7] = [
    "RUST_LOG",
    "RUST_BACKTRACE",
    "DISPLAY",
    "XAUTHORITY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
];

/// Runs `oso dev` if the first argument is `dev`, and returns its exit status.
pub fn main_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != "dev" {
        return None;
    }
    Some(run(args.collect()))
}

#[expect(
    clippy::disallowed_macros,
    reason = "`oso dev` is a command-line tool: its usage goes to stdout and stderr"
)]
fn run(args: Vec<OsString>) -> i32 {
    let file = match parse_args(args) {
        Ok(Some(file)) => file,
        Ok(None) => {
            println!("{USAGE}");
            return 0;
        }
        Err(message) => {
            eprintln!("oso dev: {message}\n\n{USAGE}");
            return 2;
        }
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("oso=info")).init();
    match supervise(file) {
        Ok(status) => status,
        Err(e) => {
            log::error!("oso dev: {e}");
            2
        }
    }
}

/// The arguments for the first instance. `None` means help was asked for.
fn parse_args(args: Vec<OsString>) -> Result<Option<Vec<OsString>>, String> {
    let mut file = None;
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
        } else if options && (arg == "-h" || arg == "--help") {
            return Ok(None);
        } else if options && arg.to_string_lossy().starts_with('-') && arg != "-" {
            return Err(format!("unknown option {}", arg.to_string_lossy()));
        } else if file.is_some() {
            return Err("only one file can be opened".into());
        } else {
            file = Some(arg);
        }
    }
    Ok(Some(file.map_or_else(Vec::new, |file| {
        vec![OsString::from("--"), file]
    })))
}

fn supervise(first_args: Vec<OsString>) -> io::Result<i32> {
    let root = std::env::current_dir()?;
    if !root.join("Cargo.toml").is_file() || !root.join("crates").is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "run oso dev from the repository root (just dev)",
        ));
    }
    let dir = ControlDir::open()?;
    let cargo = Executable::from_path("cargo", Some(&root))?;
    let binary = Executable::absolute(std::env::current_exe()?)?;
    let mut sources = Sources::new(&root);
    let mut current = Some(Running::start(&binary, &first_args, &root, &dir)?);
    let mut clean: Option<Running> = None;
    let mut retry = false;
    log::info!("oso dev: watching the sources; a failed build keeps this instance");
    loop {
        thread::sleep(POLL);
        if let Some(status) = reap(&mut current, &mut clean) {
            return Ok(status);
        }
        if !sources.changed() {
            continue;
        }
        let changed = Instant::now();
        for _ in 0..MAX_SETTLE {
            thread::sleep(POLL);
            if !sources.changed() {
                break;
            }
        }
        let started = Instant::now();
        let (built, fresh) = match build(&cargo, &root) {
            Ok(Build::Built { path, fresh }) => (path, fresh),
            Ok(Build::Failed) => {
                log::warn!("oso dev: the build failed; the running instance stays");
                continue;
            }
            Err(e) => {
                log::error!("oso dev: cannot run the build: {e}");
                continue;
            }
        };
        // Nothing to do, unless the last try left things unfinished.
        if fresh && current.is_some() && !retry {
            log::info!("oso dev: cargo rebuilt nothing");
            continue;
        }
        let binary = match Executable::absolute(built) {
            Ok(binary) => binary,
            Err(e) => {
                log::error!("oso dev: {e}");
                continue;
            }
        };
        let build_time = started.elapsed();
        if let Some(mut extra) = clean.take() {
            extra.stop();
        }
        let start = || Running::start(&binary, &[], &root, &dir).map_err(|e| e.to_string());
        retry = !restart(&mut current, &mut clean, start);
        log::info!(
            "oso dev: restart took {:?} since the change was seen: build {build_time:?}",
            changed.elapsed(),
        );
    }
}

/// Forgets instances that have exited. Returns the status to exit with once
/// the user has closed the last one. An instance that crashed is replaced by
/// the next build.
fn reap(current: &mut Option<Running>, clean: &mut Option<Running>) -> Option<i32> {
    if clean.as_mut().is_some_and(|extra| extra.exited().is_some()) {
        *clean = None;
    }
    let status = current.as_mut()?.exited()?;
    *current = clean.take();
    if current.is_some() {
        return None;
    }
    if status {
        return Some(0);
    }
    log::error!("oso dev: the instance crashed; the next good build starts a new one");
    None
}

/// Moves on to a new binary, keeping what `current` holds if there is one.
/// Returns whether a new instance runs. Reasons are logged escaped, since they
/// can quote a file name and this is a terminal.
fn restart<S>(current: &mut Option<Running>, clean: &mut Option<Running>, start: S) -> bool
where
    S: FnOnce() -> Result<Running, String>,
{
    let Some(old) = current.as_mut() else {
        return match start().and_then(|mut new| new.wait_until_ready().map(|()| new)) {
            Ok(new) => {
                *current = Some(new);
                true
            }
            Err(reason) => {
                log::warn!("oso dev: the new build did not start: {reason:?}");
                false
            }
        };
    };
    match hand_over(old, start) {
        Handover::Replaced(new) => {
            *current = Some(new);
            true
        }
        Handover::Refused { new, reason } => {
            log::warn!(
                "oso dev: the new instance refused the state, so it starts clean \
                 and the old one stays: {reason:?}"
            );
            *clean = Some(new);
            true
        }
        Handover::Failed(reason) => {
            log::warn!("oso dev: the old instance stays: {reason:?}");
            false
        }
    }
}

/// A running editor instance.
struct Running {
    child: Child,
    dir: ControlDir,
}

impl Running {
    fn start(
        binary: &Executable,
        args: &[OsString],
        root: &Path,
        dir: &ControlDir,
    ) -> io::Result<Self> {
        let child = spawn(&Spawn {
            executable: binary,
            args,
            cwd: Some(root),
            project: Some(root),
            inherit: &INSTANCE_ENV,
            stdout: Sink::Inherit,
            stderr: Sink::Inherit,
        })?;
        Ok(Self {
            child,
            dir: dir.clone(),
        })
    }

    /// Whether it has exited, and if so, whether cleanly.
    fn exited(&mut self) -> Option<bool> {
        match self.child.try_wait() {
            Ok(status) => status.map(|status| status.success()),
            Err(e) => {
                log::warn!("oso dev: cannot check on the instance: {e}");
                None
            }
        }
    }
}

impl Instance for Running {
    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let pid = Some(self.child.id());
        let line = ctl::exchange(&self.dir, pid, method, params).map_err(|e| e.to_string())?;
        protocol::response_result(&line)
    }

    fn wait_until_ready(&mut self) -> Result<(), String> {
        let started = Instant::now();
        while started.elapsed() < START_TIMEOUT {
            if self.exited().is_some() {
                return Err("it exited".into());
            }
            // The socket may not exist yet, or there may be no window yet.
            if let Ok(state) = self.call("get-state", None)
                && !state["surface"].is_null()
            {
                return Ok(());
            }
            thread::sleep(START_POLL);
        }
        Err(format!("no window after {START_TIMEOUT:?}"))
    }

    fn stop(&mut self) {
        // It may exit before answering, so the answer does not matter.
        let _ = self.call("exit", None);
        let started = Instant::now();
        while started.elapsed() < EXIT_TIMEOUT {
            if self.exited().is_some() {
                return;
            }
            thread::sleep(START_POLL);
        }
        log::warn!("oso dev: an instance did not exit in time; killing it");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What a build came to.
#[derive(Debug, PartialEq, Eq)]
enum Build {
    /// Errors, which cargo has shown in the terminal.
    Failed,
    /// The binary, here. `fresh` means cargo rebuilt nothing, as after an edit
    /// to an integration test.
    Built { path: PathBuf, fresh: bool },
}

/// Runs the incremental build. Errors go straight to the terminal, in color.
fn build(cargo: &Executable, root: &Path) -> io::Result<Build> {
    let args = [
        "build",
        "--locked",
        "--offline",
        "-p",
        "oso",
        "--features",
        "dev",
        "--message-format=json-render-diagnostics",
    ]
    .map(OsString::from);
    let mut child = spawn(&Spawn {
        executable: cargo,
        args: &args,
        cwd: Some(root),
        project: Some(root),
        inherit: &CARGO_ENV,
        stdout: Sink::Piped,
        stderr: Sink::Inherit,
    })?;
    let mut artifact = None;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            artifact = oso_artifact(&line?).or(artifact);
        }
    }
    if !child.wait()?.success() {
        return Ok(Build::Failed);
    }
    let (path, fresh) = artifact.ok_or_else(|| io::Error::other("cargo reported no oso binary"))?;
    Ok(Build::Built { path, fresh })
}

/// The oso binary's path and whether cargo rebuilt nothing, if this line of
/// cargo's JSON output reports it.
fn oso_artifact(line: &str) -> Option<(PathBuf, bool)> {
    let message: Value = serde_json::from_str(line).ok()?;
    if message["reason"] != "compiler-artifact" || message["target"]["name"] != "oso" {
        return None;
    }
    let path = message["executable"].as_str()?;
    Some((PathBuf::from(path), message["fresh"].as_bool()?))
}

#[cfg(test)]
mod tests {
    use super::{oso_artifact, parse_args};
    use serde_json::{Value, json};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn finds_the_oso_binary_in_cargos_output() {
        let line = |name: &str, fresh: bool, executable: Value| {
            json!({
                "reason": "compiler-artifact",
                "target": { "name": name },
                "executable": executable,
                "fresh": fresh,
            })
            .to_string()
        };
        assert_eq!(
            oso_artifact(&line("oso", false, json!("/t/oso"))),
            Some((PathBuf::from("/t/oso"), false))
        );
        assert_eq!(
            oso_artifact(&line("oso", true, json!("/t/oso"))),
            Some((PathBuf::from("/t/oso"), true))
        );
        assert_eq!(oso_artifact(&line("oso", false, json!(null))), None);
        assert_eq!(oso_artifact(&line("oso-core", false, json!("/t/x"))), None);
        assert_eq!(oso_artifact(r#"{"reason":"build-finished"}"#), None);
        assert_eq!(oso_artifact("not json"), None);
    }

    #[test]
    fn parses_an_optional_file() {
        let parse = |args: &[&str]| parse_args(args.iter().map(OsString::from).collect());
        assert_eq!(parse(&[]), Ok(Some(vec![])));
        assert_eq!(
            parse(&["a.txt"]),
            Ok(Some(vec![OsString::from("--"), OsString::from("a.txt")]))
        );
        assert_eq!(
            parse(&["--", "-x"]),
            Ok(Some(vec![OsString::from("--"), OsString::from("-x")]))
        );
        assert_eq!(parse(&["--help"]), Ok(None));
        assert!(parse(&["-q"]).unwrap_err().contains("unknown option"));
        assert!(parse(&["a", "b"]).unwrap_err().contains("only one"));
    }
}
