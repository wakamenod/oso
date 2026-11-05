//! Drives one running instance of the app under test over the control socket
//! (ADR 0023, ADR 0032).
//!
//! Must not depend on: winit or the app's internals; everything goes through
//! the process and the socket.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oso_platform::control::{ControlDir, ControlStream};
use serde_json::{Value, json};

use crate::process;

/// How long the app may take to listen on its socket.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a clean exit may take before the process is killed.
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
/// How often to ask whether something awaited has happened.
const POLL: Duration = Duration::from_millis(50);

/// A running app. Dropping it kills the process.
pub struct App {
    child: Child,
    reader: BufReader<ControlStream>,
    writer: ControlStream,
    next_id: u64,
    /// Wall-clock microseconds just before the process was started.
    spawned_unix_us: u64,
}

impl App {
    /// Starts `exe`, visiting `file` if given, and connects to its socket.
    pub fn start(exe: &Path, file: Option<&Path>) -> Result<Self, String> {
        let mut command = process::command(exe);
        if let Some(file) = file {
            command.arg("--").arg(file);
        }
        // Logging would change what is measured.
        command.env_remove("RUST_LOG").stdout(Stdio::null());
        let spawned_unix_us = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros(),
        )
        .unwrap_or(u64::MAX);
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
        let stream = match connect(&mut child) {
            Ok(stream) => stream,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        let reader = stream
            .try_clone()
            .map_err(|e| format!("cannot use the control socket: {e}"))?;
        Ok(Self {
            child,
            reader: BufReader::new(reader),
            writer: stream,
            next_id: 1,
            spawned_unix_us,
        })
    }

    /// Sends one request and returns its result.
    pub fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({ "id": id, "method": method });
        if let Some(params) = params {
            request["params"] = params;
        }
        writeln!(self.writer, "{request}").map_err(|e| format!("{method}: {e}"))?;
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .map_err(|e| format!("{method}: {e}"))?;
        let mut response: Value =
            serde_json::from_str(&line).map_err(|e| format!("{method}: {e}: {line:?}"))?;
        if let Some(error) = response.get("error") {
            return Err(format!("{method}: {error}"));
        }
        Ok(response["result"].take())
    }

    /// Sends keys in Emacs notation, without waiting for a frame.
    pub fn send_keys(&mut self, keys: &str) -> Result<(), String> {
        self.call("send-keys", Some(json!({ "keys": keys })))
            .map(|_| ())
    }

    /// The app's timings; see `take-timings`.
    pub fn timings(&mut self) -> Result<Value, String> {
        self.call("take-timings", None)
    }

    /// Asks for timings until `done` holds of them. Keeps no key samples
    /// that arrive meanwhile, so use it only before they matter.
    pub fn wait_until(
        &mut self,
        what: &str,
        timeout: Duration,
        done: impl Fn(&Value) -> bool,
    ) -> Result<Value, String> {
        let started = Instant::now();
        loop {
            let timings = self.timings()?;
            if done(&timings) {
                return Ok(timings);
            }
            if started.elapsed() > timeout {
                return Err(format!(
                    "no {what} after {timeout:?}; is the window hidden, or the screen locked?"
                ));
            }
            thread::sleep(POLL);
        }
    }

    /// Waits until the file is no longer counted as secret, which takes its first
    /// read. Until then keys leave no timing (ADR 0060).
    pub fn wait_until_judged(&mut self, timeout: Duration) -> Result<(), String> {
        let started = Instant::now();
        while self.call("get-state", None)?["buffer"]["secret"] != false {
            if started.elapsed() > timeout {
                return Err(format!("the file still counts as secret after {timeout:?}"));
            }
            // Faster than `POLL`: keys while loading start here, and a small file's
            // load takes tens of milliseconds.
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    /// From just before the process started to `first_frame_unix_us`, in
    /// milliseconds. This includes the time before `main`.
    pub fn spawn_to(&self, first_frame_unix_us: u64) -> f64 {
        first_frame_unix_us.saturating_sub(self.spawned_unix_us) as f64 / 1000.0
    }

    /// Asks the app to exit as a user would, so that it removes its socket,
    /// and kills it if it does not.
    pub fn stop(mut self) {
        let _ = self.send_keys("C-x C-c");
        let started = Instant::now();
        while started.elapsed() < EXIT_TIMEOUT {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(POLL);
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Does nothing if it has exited.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn connect(child: &mut Child) -> Result<ControlStream, String> {
    let started = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("the app exited at once: {status}"));
        }
        let connected = ControlDir::open_existing().and_then(|dir| dir.connect(Some(child.id())));
        match connected {
            Ok((_, stream)) => return Ok(stream),
            Err(e) if started.elapsed() > CONNECT_TIMEOUT => {
                return Err(format!("cannot connect to the app: {e}"));
            }
            Err(_) => thread::sleep(Duration::from_millis(5)),
        }
    }
}
