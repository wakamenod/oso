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
//! fails, the instance keeps running, and its echo area and `*Messages*` say why.
//! If it succeeds, and nothing holds the restart, the new binary starts and takes
//! over the old instance's state and unsaved text. Only then does the old one exit. A new binary that cannot start is closed and the
//! old one kept. One that refuses the state stays open beside the old one until
//! the next build, which closes it unless it has been edited.
//!
//! It never kills an instance, which may hold unsaved text. One that cannot be
//! stopped is left running, and the log says why. Instances run in process groups
//! of their own, so Ctrl-C here ends this and the build but leaves their windows,
//! for the user to close (ADR 0055).
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
use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
use serde_json::{Value, json};

use super::build_errors;
use super::ctl;
use super::protocol::{self, EXIT_REFUSED};
use super::restart::{CHECK, Instance, Instances, NotStopped, restart, tell};
use super::watch::Sources;

const USAGE: &str = "\
usage: oso dev [FILE]

Runs oso, rebuilds it when a Rust source changes, and restarts it with the same
buffers, unsaved text, cursors and window. Unsaved edits in a buffer that may hold
secrets hold the restart: save it, and the next build goes through. Ctrl-C ends this but leaves the window open.
Run it from the repository root (`just dev`).";

/// How often to look at the sources and the instance.
const POLL: Duration = Duration::from_millis(100);

/// The most polls to wait for a burst of saves to end before building.
const MAX_SETTLE: u32 = 10;

/// How long a new instance may take to open its window.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait between looks at a starting instance.
const START_POLL: Duration = Duration::from_millis(20);

/// How long an instance may take to exit before it is left running.
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long one that got no window may take to answer. Longer than a busy UI
/// thread takes, and short enough that those that never answer hold up no restart.
const BRIEF_TIMEOUT: Duration = Duration::from_millis(500);

/// Appears in every binary that has `oso dev`'s supervisor, and in no other.
/// `just release-safety` looks for it in release binaries, as the spawning it does
/// ships now and no longer marks a dev build (ADR 0120).
const SUPERVISOR_MARKER: &str = "oso-dev-supervisor";

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

/// What an editor instance needs beyond the base set: its log filter, how to reach the display
/// and the control directory, and where the person's configuration is (ADR 0108).
const INSTANCE_ENV: [&str; 9] = [
    "RUST_LOG",
    "RUST_BACKTRACE",
    "DISPLAY",
    "XAUTHORITY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "XDG_CONFIG_HOME",
    "APPDATA",
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

/// The arguments for the first instance. `None` means help was asked for. `--no-config` goes
/// first, and to every instance after it.
fn parse_args(args: Vec<OsString>) -> Result<Option<Vec<OsString>>, String> {
    let mut file = None;
    let mut no_config = false;
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
        } else if options && (arg == "-h" || arg == "--help") {
            return Ok(None);
        } else if options && arg == NO_CONFIG {
            no_config = true;
        } else if options && arg.to_string_lossy().starts_with('-') && arg != "-" {
            return Err(format!("unknown option {}", arg.to_string_lossy()));
        } else if file.is_some() {
            return Err("only one file can be opened".into());
        } else {
            file = Some(arg);
        }
    }
    let mut first: Vec<OsString> = no_config.then(|| NO_CONFIG.into()).into_iter().collect();
    if let Some(file) = file {
        first.extend([OsString::from("--"), file]);
    }
    Ok(Some(first))
}

/// The option that starts each instance with the defaults (ADR 0108).
const NO_CONFIG: &str = "--no-config";

/// The arguments for an instance after the first: `--no-config` if the first had it, and no file,
/// as the handoff brings the buffers.
fn restart_args(first: &[OsString]) -> Vec<OsString> {
    first
        .iter()
        .take_while(|arg| *arg != "--")
        .filter(|arg| *arg == NO_CONFIG)
        .cloned()
        .collect()
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
    // Counts the builds that made a new binary, to tell who runs the newest.
    let mut newest = 0;
    let mut instances = Instances::new(Running::start(&binary, &first_args, &root, &dir, newest)?);
    let later_args = restart_args(&first_args);
    let mut retry = false;
    log::info!(
        "oso dev: watching the sources; a failed build keeps this instance ({SUPERVISOR_MARKER})"
    );
    loop {
        thread::sleep(POLL);
        if let Some(status) = reap(&mut instances, &mut retry, newest) {
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
            Ok(Build::Failed(errors)) => {
                log::warn!("oso dev: the build failed; the running instance stays");
                let told = build_errors::messages(&errors);
                for instance in instances.current.iter_mut().chain(&mut instances.beside) {
                    told.iter().for_each(|message| tell(instance, message));
                }
                continue;
            }
            Err(e) => {
                log::error!("oso dev: cannot run the build: {e}");
                continue;
            }
        };
        // Nothing to do, unless the last try left things unfinished.
        if fresh && instances.current.is_some() && !retry {
            log::info!("oso dev: cargo rebuilt nothing");
            continue;
        }
        if !fresh {
            newest += 1;
        }
        let binary = match Executable::absolute(built) {
            Ok(binary) => binary,
            Err(e) => {
                log::error!("oso dev: {e}");
                continue;
            }
        };
        let build_time = started.elapsed();
        let start =
            || Running::start(&binary, &later_args, &root, &dir, newest).map_err(|e| e.to_string());
        retry = !restart(&mut instances, start);
        if !retry {
            log::info!(
                "oso dev: restart took {:?} since the change was seen: build {build_time:?}",
                changed.elapsed(),
            );
        }
    }
}

/// Forgets instances that have exited. Returns the status to exit with once
/// the user has closed the last one. An instance that crashed is replaced by
/// the next build. One promoted from beside it that already runs the newest binary
/// has nothing to restart for, and a restart would drop its undo history.
fn reap(instances: &mut Instances<Running>, retry: &mut bool, newest: u64) -> Option<i32> {
    instances
        .strays
        .retain_mut(|stray| stray.exited().is_none());
    if instances
        .beside
        .as_mut()
        .is_some_and(|extra| extra.exited().is_some())
    {
        instances.beside = None;
    }
    let status = instances.current.as_mut()?.exited()?;
    instances.current = instances.beside.take();
    if let Some(promoted) = &instances.current {
        *retry = *retry && promoted.build != newest;
        return None;
    }
    if status {
        for stray in &instances.strays {
            log::warn!(
                "oso dev: instance {} is left running and is yours to close",
                stray.child.id()
            );
        }
        return Some(0);
    }
    log::error!("oso dev: the instance crashed; the next good build starts a new one");
    None
}

/// A running editor instance.
struct Running {
    child: Child,
    dir: ControlDir,
    /// Which build made its binary.
    build: u64,
    /// The generation its import reported, if it took over a state.
    imported: Option<u64>,
}

impl Running {
    fn start(
        binary: &Executable,
        args: &[OsString],
        root: &Path,
        dir: &ControlDir,
        build: u64,
    ) -> io::Result<Self> {
        let child = spawn(&Spawn {
            executable: binary,
            args,
            cwd: Some(root),
            project: Some(root),
            inherit: &INSTANCE_ENV,
            stdin: Source::Closed,
            stdout: Sink::Inherit,
            stderr: Sink::Inherit,
            own_process_group: true,
            set: &[],
            terminal: None,
        })?;
        Ok(Self {
            child,
            dir: dir.clone(),
            build,
            imported: None,
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

    /// Asks it to exit if its generation is still `generation`, and gives it
    /// `limit` to do so. It may exit before answering, so only a refusal says
    /// anything. It is never killed: it may hold text that nothing else does.
    fn stop_within(&mut self, generation: u64, limit: Duration) -> Result<(), NotStopped> {
        let params = json!({ "generation": generation });
        if let Err(message) = self.call("exit", Some(params))
            && message.starts_with(EXIT_REFUSED)
        {
            return Err(NotStopped::Refused(message));
        }
        let started = Instant::now();
        while started.elapsed() < limit {
            if self.exited().is_some() {
                return Ok(());
            }
            thread::sleep(START_POLL);
        }
        Err(NotStopped::Unknown(format!(
            "instance {} did not exit within {limit:?}, so it is left running",
            self.child.id()
        )))
    }
}

impl Instance for Running {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let pid = Some(self.child.id());
        let line = ctl::exchange(&self.dir, pid, method, params).map_err(|e| e.to_string())?;
        protocol::response_result(&line)
    }

    fn check_briefly(&mut self) -> Result<Value, String> {
        let pid = Some(self.child.id());
        let line = ctl::exchange_within(&self.dir, pid, CHECK, None, BRIEF_TIMEOUT)
            .map_err(|e| e.to_string())?;
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

    fn gone(&mut self) -> bool {
        self.exited().is_some()
    }

    fn stop(&mut self, generation: u64) -> Result<(), NotStopped> {
        self.stop_within(generation, EXIT_TIMEOUT)
    }

    fn imported(&self) -> Option<u64> {
        self.imported
    }

    fn set_imported(&mut self, generation: Option<u64>) {
        self.imported = generation;
    }
}

/// What a build came to.
#[derive(Debug, PartialEq, Eq)]
enum Build {
    /// The errors, each as one row, which the terminal has shown in full.
    Failed(Vec<String>),
    /// The binary, here. `fresh` means cargo rebuilt nothing, as after an edit
    /// to an integration test.
    Built { path: PathBuf, fresh: bool },
}

/// Runs the incremental build. What the compiler says goes to the terminal, in color, as
/// cargo would show it.
fn build(cargo: &Executable, root: &Path) -> io::Result<Build> {
    let args = [
        "build",
        "--locked",
        "--offline",
        "-p",
        "oso",
        "--features",
        "dev",
        "--message-format=json-diagnostic-rendered-ansi",
    ]
    .map(OsString::from);
    let mut child = spawn(&Spawn {
        executable: cargo,
        args: &args,
        cwd: Some(root),
        project: Some(root),
        inherit: &CARGO_ENV,
        stdin: Source::Closed,
        stdout: Sink::Piped,
        stderr: Sink::Inherit,
        own_process_group: false,
        set: &[],
        terminal: None,
    })?;
    let mut artifact = None;
    let mut errors = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            let Ok(message) = serde_json::from_str::<Value>(&line?) else {
                continue;
            };
            artifact = oso_artifact(&message).or(artifact);
            if let Some(diagnostic) = build_errors::diagnostic(&message) {
                show(diagnostic.rendered.as_deref());
                errors.extend(diagnostic.error);
            }
        }
    }
    if !child.wait()?.success() {
        return Ok(Build::Failed(errors));
    }
    let (path, fresh) = artifact.ok_or_else(|| io::Error::other("cargo reported no oso binary"))?;
    Ok(Build::Built { path, fresh })
}

/// Shows what the compiler said in the terminal, as cargo would have.
#[expect(
    clippy::disallowed_macros,
    reason = "`oso dev` is a command-line tool: the compiler's output goes to stderr"
)]
fn show(rendered: Option<&str>) {
    if let Some(rendered) = rendered {
        eprint!("{rendered}");
    }
}

/// The oso binary's path and whether cargo rebuilt nothing, if this message of
/// cargo's JSON output reports it.
fn oso_artifact(message: &Value) -> Option<(PathBuf, bool)> {
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
        assert_eq!(oso_artifact(&json!({ "reason": "build-finished" })), None);
    }

    /// An instance that is a long sleep, not an editor, and whose control socket
    /// answers `exit` with `reply`. Whether the process lives shows what `stop`
    /// did to it.
    #[cfg(unix)]
    mod stopping {
        use super::super::{Instance, Instances, NotStopped, Running, reap};
        use crate::dev::protocol::EXIT_REFUSED;
        use crate::dev::server;
        use oso_platform::control::{ControlDir, Socket};
        use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
        use std::ffi::OsString;
        use std::fs::{self, Permissions};
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::time::Duration;

        /// Short, as these tests are about what happens when it runs out.
        const LIMIT: Duration = Duration::from_millis(100);

        /// A fresh control directory for one test.
        fn control_dir() -> (ControlDir, PathBuf) {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("osor-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, Permissions::from_mode(0o700)).unwrap();
            (ControlDir::at(path.join("c")).unwrap(), path)
        }

        fn sleeping(seconds: &str, dir: &ControlDir) -> Running {
            let sleep = Executable::from_path("sleep", None).unwrap();
            let child = spawn(&Spawn {
                executable: &sleep,
                args: &[OsString::from(seconds)],
                cwd: None,
                project: None,
                inherit: &[],
                stdin: Source::Closed,
                stdout: Sink::Null,
                stderr: Sink::Null,
                own_process_group: false,
                set: &[],
                terminal: None,
            })
            .unwrap();
            Running {
                child,
                dir: dir.clone(),
                build: 0,
                imported: None,
            }
        }

        /// Serves `reply` to any request, on the socket `running` is reached by.
        fn answer(running: &Running, dir: &ControlDir, reply: Result<(), &'static str>) {
            let listener = dir.listen(Socket::Dev).unwrap();
            let socket = dir.path().join(format!("{}.sock", running.child.id()));
            std::os::unix::fs::symlink(listener.socket_path(), socket).unwrap();
            server::spawn(listener, move |request: server::UiRequest| {
                let _ = request.reply.send(
                    reply
                        .map(|()| serde_json::json!({ "exiting": true }))
                        .map_err(str::to_owned),
                );
                true
            })
            .unwrap();
        }

        /// Ends the sleep, so that a test leaves nothing behind.
        fn clean_up(mut running: Running, scratch: PathBuf) {
            let _ = running.child.kill();
            let _ = running.child.wait();
            let _ = fs::remove_dir_all(scratch);
        }

        #[test]
        fn an_instance_that_refuses_is_not_killed() {
            let (dir, scratch) = control_dir();
            let mut running = sleeping("60", &dir);
            answer(&running, &dir, Err("exit refused: changed"));
            let stopped = running.stop_within(1, LIMIT);
            assert!(
                matches!(&stopped, Err(NotStopped::Refused(m)) if m.starts_with(EXIT_REFUSED)),
                "{stopped:?}"
            );
            assert!(!running.gone(), "it runs on");
            clean_up(running, scratch);
        }

        #[test]
        fn an_instance_that_does_not_answer_is_not_killed() {
            let (dir, scratch) = control_dir();
            // There is no socket to reach.
            let mut running = sleeping("60", &dir);
            let stopped = running.stop_within(1, LIMIT);
            assert!(
                matches!(&stopped, Err(NotStopped::Unknown(_))),
                "{stopped:?}"
            );
            assert!(!running.gone(), "it runs on");
            clean_up(running, scratch);
        }

        #[test]
        fn an_instance_that_agrees_and_does_not_exit_is_not_killed() {
            let (dir, scratch) = control_dir();
            let mut running = sleeping("60", &dir);
            answer(&running, &dir, Ok(()));
            let stopped = running.stop_within(1, LIMIT);
            assert!(
                matches!(&stopped, Err(NotStopped::Unknown(_))),
                "{stopped:?}"
            );
            assert!(!running.gone(), "it runs on");
            clean_up(running, scratch);
        }

        #[test]
        fn another_error_is_not_taken_for_a_refusal() {
            let (dir, scratch) = control_dir();
            let mut running = sleeping("60", &dir);
            answer(&running, &dir, Err("the editor did not reply in time"));
            let stopped = running.stop_within(1, LIMIT);
            assert!(
                matches!(&stopped, Err(NotStopped::Unknown(_))),
                "{stopped:?}"
            );
            assert!(!running.gone(), "it runs on");
            clean_up(running, scratch);
        }

        #[test]
        fn an_instance_starts_in_a_process_group_of_its_own() {
            use std::io::Read;
            let (dir, scratch) = control_dir();
            let sleep = Executable::from_path("sleep", None).unwrap();
            let running =
                Running::start(&sleep, &[OsString::from("60")], &scratch, &dir, 0).unwrap();
            let ps = Executable::from_path("ps", None).unwrap();
            let pid = running.child.id();
            let args = [
                "-o".into(),
                "pgid=".into(),
                "-p".into(),
                pid.to_string().into(),
            ];
            let mut child = spawn(&Spawn {
                executable: &ps,
                args: &args,
                cwd: None,
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
            let mut group = String::new();
            let mut stdout = child.stdout.take().unwrap();
            stdout.read_to_string(&mut group).unwrap();
            child.wait().unwrap();
            assert_eq!(group.trim(), pid.to_string(), "Ctrl-C does not reach it");
            clean_up(running, scratch);
        }

        /// The first instance has exited, and the one beside it runs `build`.
        fn promoted(build: u64, newest: u64) -> bool {
            let (dir, scratch) = control_dir();
            let mut current = sleeping("0", &dir);
            current.child.wait().unwrap();
            let mut beside = sleeping("60", &dir);
            beside.build = build;
            let mut instances = Instances::new(current);
            instances.beside = Some(beside);
            let mut retry = true;
            assert_eq!(reap(&mut instances, &mut retry, newest), None);
            let running = instances.current.take().unwrap();
            assert!(instances.beside.is_none());
            clean_up(running, scratch);
            retry
        }

        #[test]
        fn an_instance_promoted_on_the_newest_binary_needs_no_restart() {
            assert!(
                !promoted(3, 3),
                "a fresh build leaves its undo history alone"
            );
            assert!(promoted(2, 3), "it missed a build, so it is still due one");
        }

        #[test]
        fn a_stray_that_has_exited_is_forgotten_and_a_running_one_is_not() {
            let (dir, scratch) = control_dir();
            let mut ended = sleeping("0", &dir);
            ended.child.wait().unwrap();
            let mut instances = Instances::new(sleeping("60", &dir));
            instances.strays.push(ended);
            instances.strays.push(sleeping("60", &dir));
            let mut retry = false;
            assert_eq!(reap(&mut instances, &mut retry, 0), None);
            assert_eq!(instances.strays.len(), 1);
            let stray = instances.strays.pop().unwrap();
            clean_up(instances.current.take().unwrap(), scratch.clone());
            clean_up(stray, scratch);
        }

        #[test]
        fn an_instance_that_exits_has_stopped() {
            let (dir, scratch) = control_dir();
            let mut running = sleeping("0", &dir);
            assert_eq!(running.stop_within(1, Duration::from_secs(30)), Ok(()));
            assert!(running.gone());
            clean_up(running, scratch);
        }
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

    #[test]
    fn no_config_goes_to_the_first_instance_and_every_one_after() {
        let parse = |args: &[&str]| parse_args(args.iter().map(OsString::from).collect());
        let os = |args: &[&str]| args.iter().map(OsString::from).collect::<Vec<_>>();
        let first = parse(&["a.txt", "--no-config"]).unwrap().unwrap();
        assert_eq!(first, os(&["--no-config", "--", "a.txt"]));
        assert_eq!(super::restart_args(&first), os(&["--no-config"]));
        let plain = parse(&["a.txt"]).unwrap().unwrap();
        assert!(super::restart_args(&plain).is_empty());
        assert_eq!(
            parse(&["--", "--no-config"]).unwrap().unwrap(),
            os(&["--", "--no-config"]),
            "a file of that name"
        );
    }
}
