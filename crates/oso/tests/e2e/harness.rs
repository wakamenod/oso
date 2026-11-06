//! Starts real dev instances, drives them over the control socket and keeps a
//! scratch directory of generated files, for the end-to-end flows (ADR 0037,
//! ADR 0049).
//!
//! Must not depend on: the app's internals or winit; an instance is reached only
//! through its process and its socket, and is always addressed by process ID,
//! so that an instance the user has running is never touched.
//!
//! An instance is stopped when dropped, also while a test panics, and its log
//! is then printed, which `cargo test` shows for a failed test. Failure
//! messages name what was expected and never quote buffer text.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use oso_platform::control::{ControlDir, ControlStream};
use oso_platform::process::{Executable, Sink, Spawn, spawn};
use serde_json::{Value, json};

use crate::png::Png;

/// The text of the secret-pattern files, which must not leave the editor. A fake
/// one: these tests never read a real key.
pub const FAKE_SECRET: &str = "FAKE-E2E-NOT-A-REAL-SECRET";

/// How long anything awaited may take. Generous, since CI draws in software.
const TIMEOUT: Duration = Duration::from_secs(30);

/// How long an instance may take to exit when asked, while cleaning up.
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// How often to look at something awaited.
const POLL: Duration = Duration::from_millis(20);

/// How much of an instance's log to keep: the end of it.
const LOG_BYTES: usize = 64 * 1024;

/// What an instance needs beyond the base set of `oso_platform::process`: its
/// log filter, and how to reach the display and the control directory. The same
/// as for `oso dev`, whose code a test cannot import.
const INSTANCE_ENV: [&str; 7] = [
    "RUST_LOG",
    "RUST_BACKTRACE",
    "DISPLAY",
    "XAUTHORITY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
];

/// The dev build cargo built for this test.
fn oso() -> Executable {
    Executable::absolute(env!("CARGO_BIN_EXE_oso")).expect("cargo builds the binary for this test")
}

/// A running dev instance.
pub struct App {
    reader: BufReader<ControlStream>,
    writer: ControlStream,
    next_id: u64,
    dir: ControlDir,
    /// Dropped after everything else, once `drop` has asked it to exit.
    process: Process,
}

impl App {
    /// Starts an instance, visiting `file` if given, running in `cwd`, and
    /// connects to its socket. Its emergency copies go in `cwd`, whatever happens to
    /// it, and never among the real ones.
    pub fn start(file: Option<&Path>, cwd: &Path) -> Self {
        let mut args: Vec<OsString> = vec![
            "--emergency-dir".into(),
            emergency_dir(cwd).into_os_string(),
        ];
        args.extend(
            file.iter()
                .flat_map(|file| [OsString::from("--"), file.into()]),
        );
        let mut process = Process::start(&args, cwd);
        let (dir, stream) = process.connect();
        stream
            .set_read_timeout(Some(TIMEOUT))
            .expect("cannot set a timeout");
        stream
            .set_write_timeout(Some(TIMEOUT))
            .expect("cannot set a timeout");
        let reader = BufReader::new(stream.try_clone().expect("cannot use the socket"));
        let mut app = Self {
            reader,
            writer: stream,
            next_id: 1,
            dir,
            process,
        };
        let version = app.call("version", None);
        assert_eq!(
            version["pid"],
            app.pid(),
            "the socket leads to another instance"
        );
        app
    }

    /// The instance's process ID.
    pub fn pid(&self) -> u32 {
        self.process.child.id()
    }

    /// Where the instance's socket is while it runs.
    pub fn socket(&self) -> PathBuf {
        self.dir.path().join(format!("{}.sock", self.pid()))
    }

    /// Sends one request and returns the whole response line. An `Err` means
    /// no response came: the instance hung up, or took too long.
    pub fn exchange(&mut self, method: &str, params: Option<Value>) -> Result<String, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({ "id": id, "method": method });
        if let Some(params) = params {
            request["params"] = params;
        }
        writeln!(self.writer, "{request}").map_err(|e| format!("{method}: {e}"))?;
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err(format!("{method}: the instance hung up")),
            Ok(_) => Ok(line),
            Err(e) => Err(format!("{method}: {e}")),
        }
    }

    /// Sends one request and returns its result, or the error it came back
    /// with. Panics if no response came.
    pub fn try_call(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let line = self
            .exchange(method, params)
            .unwrap_or_else(|e| panic!("{e}"));
        response_result(&line).map_err(|message| format!("{method}: {message}"))
    }

    /// Sends one request and returns its result. Panics on an error response.
    pub fn call(&mut self, method: &str, params: Option<Value>) -> Value {
        self.try_call(method, params)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// Sends keys in Emacs notation. The keys have been handled when it returns.
    pub fn send_keys(&mut self, keys: &str) {
        self.call("send-keys", Some(json!({ "keys": keys })));
    }

    /// Runs a command by name. The command has run when it returns.
    pub fn run_command(&mut self, name: &str, arg: Option<i64>) -> Result<Value, String> {
        self.try_call("run-command", Some(json!({ "name": name, "arg": arg })))
    }

    /// Visits a file in the selected window.
    pub fn open_file(&mut self, path: &Path) {
        let path = path.to_str().expect("scratch paths are UTF-8");
        self.call("open-file", Some(json!({ "path": path })));
    }

    /// Shows a buffer by its id.
    pub fn show_buffer(&mut self, id: &Value) {
        self.call("show-buffer", Some(json!({ "buffer": id })));
    }

    /// The result of `get-state`.
    pub fn state(&mut self) -> Value {
        self.call("get-state", None)
    }

    /// Asks for the state until `done` holds of it, and returns that state.
    /// `what` names it if it never does.
    pub fn wait_for(&mut self, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let started = Instant::now();
        loop {
            if let Ok(Some(status)) = self.process.child.try_wait() {
                panic!("the instance exited with {status} while waiting for {what}");
            }
            let state = self.state();
            if done(&state) {
                return state;
            }
            assert!(
                started.elapsed() < TIMEOUT,
                "no {what} after {TIMEOUT:?}; the last state had surface {} and loading {}",
                state["surface"],
                state["buffer"]["loading"],
            );
            thread::sleep(POLL);
        }
    }

    /// Waits for the window, and for the file shown to finish loading, after
    /// which it is judged and its text can be read. Returns that state.
    pub fn wait_until_ready(&mut self) -> Value {
        self.wait_for("window with a loaded buffer", |state| {
            !state["surface"].is_null() && state["buffer"]["loading"].is_null()
        })
    }

    /// Takes a screenshot and reads it back.
    pub fn screenshot(&mut self) -> Png {
        let shot = self.call("screenshot", None);
        let path = shot["path"].as_str().expect("a screenshot has a path");
        let png = Png::read(Path::new(path)).unwrap_or_else(|e| panic!("the screenshot: {e}"));
        assert_eq!(
            (shot["width"].as_u64(), shot["height"].as_u64()),
            (Some(u64::from(png.width)), Some(u64::from(png.height))),
            "the reply and the file disagree on the size",
        );
        png
    }

    /// Waits for the process to end, and returns how it ended.
    pub fn wait_for_exit(&mut self) -> ExitStatus {
        let started = Instant::now();
        loop {
            let status = self.process.child.try_wait();
            if let Some(status) = status.expect("cannot look at the instance") {
                return status;
            }
            assert!(
                started.elapsed() < TIMEOUT,
                "the instance did not exit within {TIMEOUT:?}"
            );
            thread::sleep(POLL);
        }
    }

    /// Runs `oso ctl --pid PID` with `args`, and returns its exit status and
    /// standard output.
    pub fn ctl(&self, args: &[&str]) -> (ExitStatus, String) {
        let pid = self.pid().to_string();
        let args: Vec<OsString> = ["ctl", "--pid", &pid]
            .iter()
            .chain(args)
            .map(OsString::from)
            .collect();
        let mut child = spawn(&Spawn {
            executable: &oso(),
            args: &args,
            cwd: None,
            project: None,
            inherit: &["XDG_RUNTIME_DIR"],
            stdout: Sink::Piped,
            stderr: Sink::Null,
            own_process_group: false,
        })
        .expect("cannot start oso ctl");
        let mut output = String::new();
        child
            .stdout
            .take()
            .expect("stdout is piped")
            .read_to_string(&mut output)
            .expect("cannot read what oso ctl printed");
        (child.wait().expect("cannot wait for oso ctl"), output)
    }

    /// Sends `first` on this connection and at once `second` on a new one, so that
    /// both are likely to be queued together, and returns each response line. A
    /// line is empty if the instance hung up first.
    pub fn race(&mut self, first: &Value, second: &Value) -> (String, String) {
        let (_, other) = self
            .dir
            .connect(Some(self.pid()))
            .expect("cannot make a second connection");
        other
            .set_read_timeout(Some(TIMEOUT))
            .expect("cannot set a timeout");
        let mut reader = BufReader::new(other.try_clone().expect("cannot use the socket"));
        let mut writer = other;
        let _ = self.writer.write_all(format!("{first}\n").as_bytes());
        let _ = writer.write_all(format!("{second}\n").as_bytes());
        let (mut first_line, mut second_line) = (String::new(), String::new());
        let _ = self.reader.read_line(&mut first_line);
        let _ = reader.read_line(&mut second_line);
        (first_line, second_line)
    }

    /// Stops the instance and returns the end of its log, whole and raw. For
    /// looking at, not for quoting in a failure message.
    pub fn stop_and_read_log(&mut self) -> String {
        let _ = self.exchange("exit", None);
        self.wait_for_exit();
        if let Some(reader) = self.process.log_reader.take() {
            let _ = reader.join();
        }
        let log = self
            .process
            .log
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&log).into_owned()
    }
}

impl Drop for App {
    /// Asks the instance to exit, so that it removes its socket and screenshots.
    /// `Process` kills it if it has not by the end of this.
    fn drop(&mut self) {
        let _ = writeln!(self.writer, r#"{{"method":"exit"}}"#);
        let started = Instant::now();
        while started.elapsed() < EXIT_TIMEOUT
            && !matches!(self.process.child.try_wait(), Ok(Some(_)))
        {
            thread::sleep(POLL);
        }
    }
}

/// Where an instance running in `cwd` puts its emergency copies.
pub fn emergency_dir(cwd: &Path) -> PathBuf {
    cwd.join("state").join("emergency")
}

/// The process of an instance and its log, whether or not it has been reached.
struct Process {
    child: Child,
    log: Arc<Mutex<Vec<u8>>>,
    log_reader: Option<JoinHandle<()>>,
}

impl Process {
    fn start(args: &[OsString], cwd: &Path) -> Self {
        let mut child = spawn(&Spawn {
            executable: &oso(),
            args,
            cwd: Some(cwd),
            project: None,
            inherit: &INSTANCE_ENV,
            stdout: Sink::Null,
            stderr: Sink::Piped,
            own_process_group: false,
        })
        .expect("cannot start the instance");
        let log = Arc::new(Mutex::new(Vec::new()));
        let stderr = child.stderr.take().expect("stderr is piped");
        let log_reader = {
            let log = Arc::clone(&log);
            thread::spawn(move || keep_log(stderr, &log))
        };
        Self {
            child,
            log,
            log_reader: Some(log_reader),
        }
    }

    /// Connects to the instance's socket, and panics if it exits or never
    /// listens.
    fn connect(&mut self) -> (ControlDir, ControlStream) {
        let pid = self.child.id();
        let started = Instant::now();
        loop {
            let status = self.child.try_wait().expect("cannot look at the instance");
            if let Some(status) = status {
                panic!("the instance exited with {status} before listening");
            }
            assert!(
                started.elapsed() < TIMEOUT,
                "the instance did not listen within {TIMEOUT:?}"
            );
            // The directory may not exist yet, on a first run.
            if let Ok(dir) = ControlDir::open_existing()
                && let Ok((_, stream)) = dir.connect(Some(pid))
            {
                return (dir, stream);
            }
            thread::sleep(POLL);
        }
    }
}

impl Drop for Process {
    /// Kills the instance if it is still there, removes what it left, and shows
    /// the log of a failed test. `cargo test` hides that of a passing one.
    fn drop(&mut self) {
        // Does nothing if it has exited.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let pid = self.child.id();
        if let Ok(dir) = ControlDir::open_existing() {
            remove_files_of(pid, dir.path());
        }
        // After the instance is gone, so that the whole log shows.
        if let Some(reader) = self.log_reader.take() {
            let _ = reader.join();
        }
        if thread::panicking() {
            print_log(pid, &self.log);
        }
    }
}

/// Removes the socket and files of the instance `pid` from the control
/// directory, and nobody else's. An instance that exited cleanly has none left.
fn remove_files_of(pid: u32, dir: &Path) {
    let (socket, prefix) = (format!("{pid}.sock"), format!("{pid}-"));
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if *name == *socket || name.starts_with(&prefix) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Prints a log, without the fake secret in case a bug put it there.
#[expect(
    clippy::disallowed_macros,
    reason = "a test's output, which `cargo test` captures"
)]
fn print_log(pid: u32, log: &Mutex<Vec<u8>>) {
    let log = log.lock().unwrap_or_else(PoisonError::into_inner);
    let log = String::from_utf8_lossy(&log).replace(FAKE_SECRET, "[the fake secret]");
    eprintln!("--- the log of instance {pid} ---\n{log}--- end of the log ---");
}

/// Reads a log until the instance closes it, keeping the end.
fn keep_log(stderr: impl Read, log: &Mutex<Vec<u8>>) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
        let mut log = log.lock().unwrap_or_else(PoisonError::into_inner);
        log.append(&mut line);
        if log.len() > LOG_BYTES {
            let extra = log.len() - LOG_BYTES;
            log.drain(..extra);
        }
    }
}

/// The result in a response line, or its error message. Keeps the line itself
/// out of the message, since it can hold buffer text.
fn response_result(line: &str) -> Result<Value, String> {
    let mut response: Value =
        serde_json::from_str(line).map_err(|e| format!("not a JSON response: {e}"))?;
    if let Some(message) = response["error"]["message"].as_str() {
        return Err(message.to_owned());
    }
    match response.get_mut("result") {
        Some(result) => Ok(result.take()),
        None => Err("a response with neither a result nor an error".to_owned()),
    }
}

/// A directory for one test's files, removed when dropped. Everything in it is
/// generated by the test.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// A new directory named after the test, with no links in its path.
    pub fn new(test: &str) -> Self {
        let name = format!("oso-e2e-{}-{test}", std::process::id());
        let temp = fs::canonicalize(std::env::temp_dir()).expect("no temporary directory");
        let path = temp.join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).expect("cannot create the scratch directory");
        // So that what a private directory checks does not depend on the umask.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("cannot set the scratch directory's mode");
        Self { path }
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes a file at `name`, relative to the directory, creating the
    /// directories on the way, and returns its path.
    pub fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        let parent = path.parent().expect("a file has a parent");
        fs::create_dir_all(parent).expect("cannot create a scratch directory");
        fs::write(&path, bytes).expect("cannot write a scratch file");
        path
    }

    /// Makes a symbolic link at `name`, relative to the directory, to `target`,
    /// and returns its path.
    pub fn symlink(&self, name: &str, target: &Path) -> PathBuf {
        let path = self.path.join(name);
        std::os::unix::fs::symlink(target, &path).expect("cannot make a symbolic link");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Asks the app with process ID `pid` to quit, as the Dock's Quit does, and says whether
/// the request could be sent.
#[cfg(target_os = "macos")]
pub fn ask_to_quit(pid: u32) -> bool {
    let osascript = Executable::from_path("osascript", None).expect("osascript is on PATH");
    let script = format!(
        "ObjC.import('AppKit'); $.NSRunningApplication.runningApplicationWithProcessIdentifier({pid}).terminate"
    );
    let args = ["-l".into(), "JavaScript".into(), "-e".into(), script.into()];
    let status = spawn(&Spawn {
        executable: &osascript,
        args: &args,
        cwd: None,
        project: None,
        inherit: &[],
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
    })
    .and_then(|mut child| child.wait())
    .expect("cannot run osascript");
    status.success()
}

/// Whether a process with this ID exists, as `ps` sees it.
pub fn process_exists(pid: u32) -> bool {
    let ps = Executable::from_path("ps", None).expect("ps is on PATH");
    let args = ["-p".into(), pid.to_string().into()];
    let status = spawn(&Spawn {
        executable: &ps,
        args: &args,
        cwd: None,
        project: None,
        inherit: &[],
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
    })
    .and_then(|mut child| child.wait())
    .expect("cannot run ps");
    status.success()
}
