//! What a session starts in, once it is ready (ADR 0121, ADR 0165): its terminal, or a native
//! session's `claude -p` with pipes for its stream.
//!
//! Must not depend on: `app` or the UI thread; it runs on the session's start thread.

use std::ffi::OsString;
use std::path::Path;

use oso_core::buffer::BufferId;
use oso_platform::process::{self, Sink, Source, Spawn};
use oso_term::Terminal;

use super::super::launch::{self, Launch};
use super::super::past;
use super::super::stream::{Out, Stream};
use super::{NativeOrder, Runs, SPAWN_WAIT, Started};

/// Starts the session as `runs` says: in its terminal, or a native session's `claude -p`.
pub(super) fn start_runs(
    runs: Runs,
    launch: &Launch,
    directory: &Path,
    project: Option<&Path>,
    set: Vec<(String, OsString)>,
    brief: Option<String>,
) -> Result<(BufferId, Started), String> {
    match runs {
        Runs::Terminal(given) => {
            let start = oso_term::Start {
                executable: launch.program.clone(),
                args: launch.args.clone(),
                cwd: Some(directory.to_path_buf()),
                project: project.map(Path::to_path_buf),
                inherit: launch::inherit(&given.settings.env, &given.env),
                set,
                size: given.size,
                scrollback: given.settings.scrollback,
                palette: given.palette,
                // Its name may come from a transcript, its brief and instructions from a person
                // or an agent: none of them reaches the log (ADR 0145).
                args_logged: false,
            };
            let terminal = Terminal::start(start, given.ready)
                .map_err(|e| format!("The session's terminal did not start: {e}"))?;
            // `start` returns before the child is spawned: the session's record has its id from
            // the start, or check 4 and the registry's poll take its `claude` for another's. The
            // UI reads it when it takes the terminal, so that a child reaped meanwhile gives none.
            terminal.wait_started(SPAWN_WAIT);
            Ok((given.buffer, Started::Terminal(terminal)))
        }
        Runs::Native(given) => start_native(launch, directory, project, &set, brief, given)
            .map_err(|e| format!("The session did not start: {e}")),
    }
}

/// Starts a native session's `claude -p` (ADR 0165): with the variables a terminal's would
/// have, pipes for its stream, in a session of its own, and its first message, `brief`, on
/// its standard input. Once it is spawned, the earlier turns its order asks for are read and
/// handed on first (step 5).
fn start_native(
    launch: &Launch,
    directory: &Path,
    project: Option<&Path>,
    set: &[(String, OsString)],
    brief: Option<String>,
    given: NativeOrder,
) -> std::io::Result<(BufferId, Started)> {
    let names = launch::inherit(&given.settings.env, &given.env);
    let inherit: Vec<&str> = names.iter().map(String::as_str).collect();
    let set: Vec<(&str, &std::ffi::OsStr)> = (set.iter())
        .map(|(name, value)| (name.as_str(), value.as_os_str()))
        .collect();
    let child = process::spawn(&Spawn {
        executable: &launch.program,
        args: &launch.args,
        cwd: Some(directory),
        project,
        inherit: &inherit,
        stdin: Source::Piped,
        stdout: Sink::Piped,
        stderr: Sink::Piped,
        own_process_group: false,
        own_session: true,
        set: &set,
        terminal: None,
        pass_fd: None,
        // As a terminal's: its name, brief and instructions never reach the log.
        args_logged: false,
    })?;
    let mut out = given.out;
    // Its earlier turns go before anything its process says (ADR 0165, step 5).
    if let Some(ask) = &given.past {
        out(Out::Past(Box::new(past::read(ask, directory))));
    }
    let stream = Stream::run(child, brief.as_deref(), out)?;
    Ok((given.buffer, Started::Native(stream)))
}
