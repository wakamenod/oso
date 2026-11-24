//! A one-shot `claude -p`: the person's own `claude`, run once through their shell with no
//! tool, no project setting and no session kept, in a new, empty, private directory, given its
//! input on standard input and killed with its whole session past a limit. A new space's names
//! (ADR 0159) and a dictation's fix (ADR 0175, step 2) are asked this way.
//!
//! Must not depend on: `app`; the caller gives the arguments, the input and the limits, and
//! runs [`run`] on a thread of its own.
//!
//! - [`arguments`] gives every run the options that keep it to itself: `--safe-mode` leaves out
//!   CLAUDE.md, skills, plugins, hooks, MCP servers and custom agents but keeps the login and
//!   the model, `--tools=` gives no built-in tool, and nothing is kept of the session. No
//!   permission option is given: with no tool, nothing is asked.
//! - [`structured`] reads what it answered, in one place for every caller.
//! - The input is the one text from outside: it goes on standard input, never in an argument.
//! - No agent's socket nor oso's goes to it, and no `SSH_AUTH_SOCK`.
//! - The directory goes once the run ended, unless the run wrote there.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ChildStdout;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::process::{Child, Signal, Sink, Source, Spawn, spawn};
use serde_json::Value;

use crate::sessions::launch;

/// The options every run gets, each as `--flag=value` so that no value is taken for an
/// option.
const ISOLATED: [&str; 8] = [
    "--print",
    "--safe-mode",
    "--tools=",
    "--strict-mcp-config",
    "--setting-sources=user",
    "--disable-slash-commands",
    "--no-session-persistence",
    "--output-format=json",
];

/// The option that keeps a run off claude.ai, whatever the person's settings say: it is no
/// session to follow there (ADR 0181).
fn remote_control_off() -> OsString {
    format!(
        "--settings={}",
        crate::sessions::settings::remote_control_off()
    )
    .into()
}

/// A run's arguments: [`ISOLATED`]'s, Remote Control off, then the caller's `extra`, which are options too.
pub fn arguments(extra: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    ISOLATED
        .into_iter()
        .map(OsString::from)
        .chain([remote_control_off()])
        .chain(extra)
        .collect()
}

/// Why nothing can be read from what a run wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unread {
    /// It was not JSON.
    NotJson,
    /// `claude` said it failed.
    Failed,
    /// It held no object: no `structured_output`, and no `result` that reads as one.
    Empty,
}

/// What a run answered, `bytes`, `--output-format=json`'s: its `structured_output`, else its
/// `result` read as JSON.
///
/// # Errors
///
/// Why nothing can be read from it.
pub fn structured(bytes: &[u8]) -> Result<Value, Unread> {
    let mut value: Value = serde_json::from_slice(bytes).map_err(|_| Unread::NotJson)?;
    if value.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err(Unread::Failed);
    }
    if let Some(object @ Value::Object(_)) = value.get_mut("structured_output") {
        return Ok(object.take());
    }
    let result = value
        .get("result")
        .and_then(Value::as_str)
        .ok_or(Unread::Empty)?;
    serde_json::from_str(result).map_err(|_| Unread::Empty)
}

/// How often the run looks whether `claude` ended.
const POLL: Duration = Duration::from_millis(20);

/// The run's `claude` while it runs, shared with whoever may stop it: an exit or a restart
/// stops it with [`stop`], as nothing else would before its limit. A stop before the run
/// started holds: it then starts nothing.
pub type Running = Arc<Mutex<Run>>;

/// What [`Running`] holds.
#[derive(Debug, Default)]
pub struct Run {
    child: Option<Child>,
    /// Asked to stop: no `claude` starts, and one that started as the stop came is killed.
    stopped: bool,
}

impl Run {
    /// Whether its `claude` has started and is not yet reaped, for the stand-in runs' tests.
    #[cfg(all(test, unix))]
    pub fn started(&self) -> bool {
        self.child.is_some()
    }
}

/// One run.
pub struct Oneshot<'a> {
    /// The person's terminal settings, whose shell runs `claude` as a session's does.
    pub terminal: &'a crate::terminal::shell::Settings,
    /// `[sessions] claude`, if set.
    pub claude: Option<&'a Path>,
    /// `[sessions] env`: the names a session takes from oso's environment.
    pub session_env: &'a [String],
    /// oso's state directory.
    pub state: &'a Path,
    /// The directory in it whose new directory `name` the run starts in.
    pub directory: &'a str,
    /// The run's own directory's name: one at a time for each.
    pub name: &'a str,
    /// `claude`'s arguments, options alone.
    pub arguments: Vec<OsString>,
    /// What `claude` reads on its standard input; small enough to fit in a pipe.
    pub input: &'a [u8],
    /// How long it may take.
    pub limit: Duration,
    /// The most bytes of the answer read.
    pub max_answer: usize,
    /// Where the run keeps its `claude` while it runs.
    pub running: &'a Running,
}

/// The directories in oso's state directory that runs start in: a space's naming's
/// (ADR 0159) and a dictation's fix's (ADR 0175).
const DIRECTORIES: [&str; 2] = [
    crate::sessions::spaces::summary::DIRECTORY,
    crate::dictation::fix::DIRECTORY,
];

/// Leaves out of `live`, the sessions Claude Code's registry lists, the runs in the
/// directories runs start in, in the state directory `state`: each is oso's own, and no session
/// of the person's.
pub fn leave_out_runs(
    state: Option<PathBuf>,
    live: &mut Vec<crate::sessions::worker::externals::External>,
) {
    let Some(state) = state else {
        return;
    };
    let runs = DIRECTORIES.map(|directory| state.join(directory));
    live.retain(|external| {
        !(external.directory.as_deref())
            .is_some_and(|dir| runs.iter().any(|run| dir.starts_with(run)))
    });
}

/// Runs `oneshot`'s `claude` in a new private directory, and gives what it wrote. Runs on the
/// caller's thread, which it holds as long as the run.
///
/// # Errors
///
/// Why no answer came, for the echo area: it names no path and holds none of the input.
pub fn run(oneshot: &Oneshot<'_>) -> Result<Vec<u8>, String> {
    let directory = fresh_directory(oneshot.state, oneshot.directory, oneshot.name)?;
    let answered = run_in(oneshot, &directory);
    // Left as it was made, empty, unless the run wrote there: then it stays, and says so.
    if std::fs::remove_dir(&directory).is_err() {
        log::warn!(
            "a run of claude left its directory in oso's state directory, under {}",
            oneshot.directory
        );
    }
    answered
}

/// The run of `oneshot`'s `claude` in `directory`.
fn run_in(oneshot: &Oneshot<'_>, directory: &Path) -> Result<Vec<u8>, String> {
    let shell = crate::terminal::hosted::find_shell(oneshot.terminal)
        .map_err(|_| "no shell is found".to_owned())?;
    let claude = launch::configured(oneshot.claude)?;
    if lock(oneshot.running).stopped {
        return Err("oso stopped it".to_owned());
    }
    let launch = launch::launch(
        &shell.executable,
        claude.as_ref(),
        oneshot.arguments.clone(),
        None,
    )
    .map_err(|e| e.to_string())?;
    // No agent's socket nor oso's: the run has no tool and no MCP server.
    let names = launch::claude_inherit(&oneshot.terminal.env, oneshot.session_env);
    let inherit: Vec<&str> = (names.iter().map(String::as_str))
        .filter(|&name| name != "SSH_AUTH_SOCK")
        .collect();
    let mut child = spawn(&Spawn {
        executable: &launch.program,
        args: &launch.args,
        cwd: Some(directory),
        project: Some(directory),
        inherit: &inherit,
        stdin: Source::Piped,
        stdout: Sink::Piped,
        stderr: Sink::Null,
        own_process_group: false,
        own_session: true,
        set: &[],
        terminal: None,
        pass_fd: None,
        args_logged: false,
    })
    .map_err(|e| format!("claude did not start: {e}"))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    {
        let mut run = lock(oneshot.running);
        if run.stopped {
            // Stopped while it started: it never gets its input.
            drop(run);
            let _ = child.signal_group(Signal::Kill);
            let _ = child.kill();
            let _ = child.wait();
            return Err("oso stopped it".to_owned());
        }
        run.child = Some(child);
    }
    // Written on a thread of its own, so that an input larger than the pipe never holds this
    // one past the limit; it ends when `claude` reads it or goes.
    if let Some(mut stdin) = stdin {
        let input = oneshot.input.to_vec();
        let writer = thread::Builder::new()
            .name("oso-oneshot-write".into())
            .spawn(move || {
                let _ = stdin.write_all(&input);
            });
        if writer.is_err() {
            stop(oneshot.running);
            return Err("it could not be given its input".to_owned());
        }
    }
    let answered = stdout
        .ok_or_else(|| "it could not be read".to_owned())
        .and_then(|stdout| wait(oneshot, stdout));
    // Stopped already, or ended: nothing of it is left to stop.
    let stopped = lock(oneshot.running).stopped;
    stop(oneshot.running);
    if stopped {
        return Err("oso stopped it".to_owned());
    }
    answered
}

/// Waits up to the limit for the `claude` in `oneshot.running` to end and for at most
/// `max_answer` bytes of its output, `stdout`. Past the limit its whole session is killed, so
/// that nothing it started outlives it and holds its output open.
fn wait(oneshot: &Oneshot<'_>, stdout: ChildStdout) -> Result<Vec<u8>, String> {
    let limit = oneshot.limit;
    let deadline = Instant::now() + limit;
    let late = || format!("no answer in {} s", limit.as_secs());
    let (sent, got) = mpsc::channel();
    let max = oneshot.max_answer;
    let reader = thread::Builder::new()
        .name("oso-oneshot-read".into())
        .spawn(move || {
            let _ = sent.send(crate::git::read_at_most(stdout, max));
        });
    if reader.is_err() {
        return Err("it could not be read".to_owned());
    }
    let status = loop {
        let looked = lock(oneshot.running).child.as_mut().map(Child::try_wait);
        match looked {
            Some(Ok(Some(status))) => break status,
            Some(Ok(None)) if Instant::now() < deadline => thread::sleep(POLL),
            // Stopped by an exit, or past its limit.
            None => return Err("oso stopped it".to_owned()),
            _ => return Err(late()),
        }
    };
    let left = deadline.saturating_duration_since(Instant::now()).max(POLL);
    let (bytes, cut) = match got.recv_timeout(left) {
        Ok(Ok(read)) => read,
        // What it started holds its output open past the limit.
        _ => return Err(late()),
    };
    if cut {
        return Err("its answer was too long".to_owned());
    }
    if !status.success() {
        return Err("claude did not answer".to_owned());
    }
    Ok(bytes)
}

/// Stops the run in `running`: none starts after this, and a `claude` that is there has its
/// whole session killed, then it, and it is reaped. An exit and a restart do this, and a run
/// past its limit.
pub fn stop(running: &Running) {
    let child = {
        let mut run = lock(running);
        run.stopped = true;
        run.child.take()
    };
    if let Some(mut child) = child {
        let _ = child.signal_group(Signal::Kill);
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Kills the `claude` in `running`, if one is there, with its whole session, and waits for
/// nothing: the run's own thread, which looks every [`POLL`], sees it end and reaps it. For the
/// UI thread, which must not wait on a process.
pub fn kill(running: &Running) {
    let mut run = lock(running);
    run.stopped = true;
    if let Some(child) = run.child.as_mut() {
        let _ = child.signal_group(Signal::Kill);
        let _ = child.kill();
    }
}

/// `running`'s lock, whether or not a holder panicked: the child and a flag are all it holds.
fn lock(running: &Running) -> std::sync::MutexGuard<'_, Run> {
    running
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A new directory for one run, `name` under `directory` in `state`, both made private: a
/// directory already there, which a file in could be a setting `claude` reads, is not used.
fn fresh_directory(state: &Path, directory: &str, name: &str) -> Result<PathBuf, String> {
    let refused = || "its directory may not be used".to_owned();
    let parent =
        oso_platform::privacy::PrivateDir::open(state.join(directory)).map_err(|_| refused())?;
    let path = parent.path().join(name);
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err("its directory is there already".to_owned());
    }
    let private = oso_platform::privacy::PrivateDir::open(&path).map_err(|_| refused())?;
    Ok(private.path().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::{Unread, arguments, structured};
    use serde_json::json;

    #[test]
    fn every_run_is_kept_to_itself_and_the_callers_options_follow() {
        let args = arguments(["--model=haiku".into()]);
        assert_eq!(args[0], "--print");
        assert_eq!(args.last().unwrap(), "--model=haiku");
        for flag in [
            "--safe-mode",
            "--tools=",
            "--strict-mcp-config",
            "--no-session-persistence",
            r#"--settings={"disableRemoteControl":true}"#,
        ] {
            assert!(args.iter().any(|arg| arg == flag), "{flag}");
        }
    }

    #[test]
    fn the_answer_is_its_structured_output_else_its_result() {
        let both = json!({"result": "{}", "structured_output": {"a": 1}}).to_string();
        assert_eq!(structured(both.as_bytes()), Ok(json!({"a": 1})));
        let result = json!({"result": "{\"b\": 2}"}).to_string();
        assert_eq!(structured(result.as_bytes()), Ok(json!({"b": 2})));
        let failed = json!({"is_error": true, "structured_output": {"a": 1}}).to_string();
        assert_eq!(structured(failed.as_bytes()), Err(Unread::Failed));
        assert_eq!(structured(b"no"), Err(Unread::NotJson));
        assert_eq!(structured(b"{\"result\": \"text\"}"), Err(Unread::Empty));
    }
}
