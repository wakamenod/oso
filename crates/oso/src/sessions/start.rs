//! Getting a hosted session ready to start, on a short `oso-session-start` thread of its own
//! (ADR 0121, "A session" and "Restore and the fork hazard"): everything that touches the file
//! system or the process table before `claude` runs, so the UI thread never waits on it.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; the caller gives an [`Order`] and
//! takes [`Report`]s through the function it gives.
//!
//! - A new session's id is made from 16 random bytes from the system; a fork gets a new one too.
//! - A resume first reads Claude Code's registry: a live process with the id refuses, naming
//!   its process and directory, and the person is offered a fork. Five seconds after the
//!   resume, the registry is read again, and two live processes with the id give a warning.
//!   These checks are `live`'s. Before them, a native resume of a session whose TUI's
//!   `claude` may still run waits for it to be gone (`gone`, ADR 0165, part 4d-2).
//! - A TUI's `claude`'s start time is read just after its spawn, for that wait.
//! - A missing directory refuses before anything starts.
//! - A native resume, fork or adoption judged by the record of trusted projects (ADR 0165,
//!   parts 4a and 4e) is judged here by its root's path and identity, before the root's other
//!   checks.
//! - For a new or adopted session in a trusted project, the baseline is taken first (ADR 0122), waiting at most
//!   [`BASELINE_WAIT`]; past that, the session starts and the baseline follows (`baseline`).
//! - The shell is found as ADR 0120 has it, and `claude` on `PATH` when the shell is not one
//!   that runs the wrapper, as [`launch`](super::launch) has it.
//! - The terminal starts here too, with ADR 0120's environment, `[sessions] env`'s names and
//!   `CLAUDE_CONFIG_DIR` and `OSO_SESSION`, so that the UI thread never waits on the spawn.
//!   The thread waits for the spawn, at most [`SPAWN_WAIT`], before it reports [`Ready`]: oso's
//!   record has its own `claude`'s process id from its first check on.
//! - A native session (ADR 0165) starts `claude -p` here instead, with the same variables and
//!   pipes for its stream, in a session of its own; [`stream`](super::stream) reads it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use oso_core::buffer::BufferId;
use oso_platform::process::Executable;
use oso_platform::process::pty::Size;
use oso_term::{Palette, Terminal};

use super::command::{self, Line, SESSION_VARIABLE, Start};
use super::identity::{self, SessionId};
use super::launch::{self, Launch};
use super::stream::{self, Stream};

mod baseline;
pub mod gone;
mod live;
mod runs;
pub mod space_gate;
pub mod trust;

use super::worktree::steer;
use crate::git::TrustedRoot;
use crate::terminal::hosted;
use crate::terminal::shell::{self, Settings};
use baseline::Baseline;
use live::{held_back, live_count};
use runs::start_runs;

/// The longest a session waits for its baseline before it starts (ADR 0122).
pub const BASELINE_WAIT: Duration = Duration::from_secs(2);

/// The longest a baseline is waited for at all, after the session started: git's own runs are
/// bounded, so this only ends a wait that went wrong.
const BASELINE_GIVE_UP: Duration = Duration::from_secs(60);

/// How long the start thread waits for the terminal's thread to spawn the child before it
/// reports the start anyway, with no process id yet; a bridge's claim reads it again.
pub const SPAWN_WAIT: Duration = Duration::from_secs(5);

/// How long after a resume the registry is read again (ADR 0121).
pub const RECHECK_AFTER: Duration = Duration::from_secs(5);

/// What a session starts as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    New,
    Resume(SessionId),
    Fork(SessionId),
}

/// What the UI thread asks for.
pub struct Order {
    /// The UI's name for this start, given back in each report.
    pub token: u64,
    pub kind: Kind,
    pub name: String,
    pub directory: PathBuf,
    /// The project it runs in, which the session's executables are not looked for in.
    pub project: Option<PathBuf>,
    /// The project's root, if the project is trusted to run git: a native session's gate,
    /// looked at again before the spawn, and the baseline's root.
    pub trusted: Option<TrustedRoot>,
    /// Whether a baseline is taken at `trusted` before the session starts: a new session's
    /// and an adopted one's (ADR 0122), never a resume's or a fork's.
    pub baseline: bool,
    /// oso's own state directory, where reviews keep their index copies.
    pub state: Option<PathBuf>,
    /// `[sessions] claude`, an absolute path.
    pub claude: Option<PathBuf>,
    /// What it runs in: a terminal, or for a native session, pipes.
    pub runs: Runs,
    /// The first prompt: a worktree session's brief with its header (ADR 0121 step 4). A
    /// native session's goes on its standard input.
    pub brief: Option<String>,
    /// `[sessions] steer_worktrees`: the session gets the hook that steers Claude away from
    /// its own worktree tools.
    pub steer_worktrees: bool,
    /// Whether to ask first when another live session runs in the directory.
    pub check_shared: bool,
    /// An issue space's orchestrator: what it starts with and what its directory must pass
    /// (ADR 0136, step 3).
    pub space: Option<SpaceStart>,
}

/// What an orchestrator starts with, looked at again just before it starts (ADR 0136, step 3).
#[derive(Debug, Clone)]
pub struct SpaceStart {
    /// Whether it starts in the space's own directory, which its checks look at first; a
    /// worker in a repository gets none of them (ADR 0136, step 4b).
    pub in_space_directory: bool,
    /// The space directory's identity when oso made it.
    pub made: Option<(u64, u64)>,
    /// Every repository of the space, which the directory must be inside none of.
    pub members: Vec<crate::sessions::spaces::Member>,
    /// The trusted ones, given with `--add-dir` while each is still the one recorded.
    pub trusted: Vec<crate::sessions::spaces::Member>,
    /// For a worker, the directories it also reads, each with its repository's trust: each is
    /// checked again here, with git, as the tool's job checked it (ADR 0136, step 4b).
    pub reads: Vec<(crate::sessions::worktree::jobs::Trust, PathBuf)>,
    /// The fixed text and the space's header, for `--append-system-prompt`.
    pub prompt: String,
    /// The space's instructions, read here and added to the prompt.
    pub instructions: Option<PathBuf>,
}

/// What a session runs in.
pub enum Runs {
    Terminal(TerminalOrder),
    Native(NativeOrder),
}

impl Runs {
    /// `[terminal]`'s settings, whose shell starts the session either way.
    fn settings(&self) -> &Settings {
        match self {
            Self::Terminal(order) => &order.settings,
            Self::Native(order) => &order.settings,
        }
    }

    fn native(&self) -> bool {
        matches!(self, Self::Native(_))
    }
}

/// A native session's start (ADR 0165), as the app gives it.
pub struct NativeOrder {
    /// Its buffer, already made.
    pub buffer: BufferId,
    /// The record of trusted projects it is judged by here, with no open buffer of its
    /// project (ADR 0165, part 4a).
    pub trust: trust::ByRecord,
    /// Its gate in an issue space's own directory, which it runs in instead of a trusted
    /// project's root (ADR 0176): its line then reads the person's own settings alone.
    pub gate: space_gate::ByGate,
    /// `[terminal]`'s settings: the shell, `-l` and the names passed on.
    pub settings: Settings,
    /// `[sessions] env`: more names passed on from oso's environment.
    pub env: Vec<String>,
    /// Called from the stream's threads with what they read.
    pub out: stream::Deliver,
    /// The permission mode it starts in, `--permission-mode` (ADR 0165, part 3b): the mode a
    /// resumed session was in or was switched to; `None` for the person's settings' own.
    pub permission_mode: Option<crate::channel::protocol::mode::PermissionMode>,
    /// The model and effort level it starts with, `--model` and `--effort` (part 3c): what the
    /// person chose with `/model` and `/effort` in a process before; `None` for the settings'.
    pub model: Option<String>,
    pub effort: Option<&'static str>,
    /// The conversation whose earlier turns are read from its transcript once its process is
    /// spawned, and handed on before its stream (step 5): a resume's own, or the one a fork
    /// copies, all of it or what follows the rows' newest messages (part 5b); `None` for a new
    /// conversation, or rows its conversation cannot have gone on from.
    pub past: Option<crate::sessions::native::past::PastAsk>,
    /// The session's TUI `claude`, which may still run: the start waits for it to be gone
    /// first (part 4d-2).
    pub tui_to_end: Option<super::hosted::TuiProcess>,
}

/// The terminal a session runs in, as the app gives it.
pub struct TerminalOrder {
    /// Its buffer, already made.
    pub buffer: BufferId,
    /// `[terminal]`'s settings: the shell, `-l`, the names passed on and the scrollback.
    pub settings: Settings,
    /// `[sessions] env`: more names passed on from oso's environment.
    pub env: Vec<String>,
    pub size: Size,
    pub palette: Palette,
    /// Called from the terminal's thread whenever a snapshot is ready.
    pub ready: Box<dyn Fn() + Send>,
    /// What the native session handed to this TUI held (ADR 0165, part 4d): its permission
    /// mode, model and effort, `None` for any other terminal's start, which passes none.
    pub handed: Option<Chosen>,
}

/// The permission mode, model and effort a native session held when it is handed to the TUI
/// (ADR 0165, part 4d, the author's Q4-7 (a)), passed as `--permission-mode`, `--model` and
/// `--effort`, as a native session's own processes are given them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chosen {
    pub permission_mode: Option<crate::channel::protocol::mode::PermissionMode>,
    pub model: Option<String>,
    pub effort: Option<&'static str>,
}

/// A session whose terminal started.
#[derive(Debug)]
pub struct Ready {
    pub token: u64,
    pub id: SessionId,
    pub buffer: BufferId,
    /// A terminal past its child's start, unless that took longer than [`SPAWN_WAIT`]: its
    /// `pid` is the child's while it runs. Or a native session's stream.
    pub started: Started,
    /// The shell's path and the shells passed over before it, for `*Messages*`.
    pub shell: PathBuf,
    pub passed_over: Vec<String>,
    pub directory: PathBuf,
    /// The project it runs in, as the order gave it.
    pub project: Option<PathBuf>,
    /// Its baseline's tree id, if it was taken in time.
    pub baseline: Option<String>,
    /// What the echo area says of the start beside it: an orchestrator's repository left out.
    pub notes: Vec<String>,
    /// A terminal's child's process id and when it started, as the system said just after
    /// the spawn, for a later native resume's wait (ADR 0165, part 4d-2).
    pub child_started: Option<(u32, std::time::SystemTime)>,
}

/// What a session started in.
#[derive(Debug)]
pub enum Started {
    Terminal(Terminal),
    Native(Stream),
}

/// What the thread found.
#[derive(Debug)]
pub enum Report {
    /// Start it. Boxed: the process handles make it far larger than the rest.
    Ready(Box<Ready>),
    /// It does not start; why, for the echo area.
    Refused { token: u64, why: String },
    /// A resume found the conversation live in this process, in this directory; or, with no
    /// process, could not tell from the registry whether it runs, which counts as live.
    Live {
        token: u64,
        pid: Option<u32>,
        directory: Option<PathBuf>,
    },
    /// The baseline came after the session started.
    Baseline { token: u64, tree: String },
    /// Two live processes have the id of the session started with `token`.
    Twice { token: u64 },
    /// Another live session runs in the directory: nothing started, and the person is asked.
    Shared { token: u64 },
}

/// Starts the thread for `order`; it calls `send` with each report. Refused at once if no
/// thread starts.
pub fn spawn(order: Order, send: impl Fn(Report) + Send + Sync + 'static) {
    let token = order.token;
    let send = std::sync::Arc::new(send);
    let sends = std::sync::Arc::clone(&send);
    let spawned = thread::Builder::new()
        .name("oso-session-start".into())
        .spawn(move || prepare(order, &*sends));
    if let Err(e) = spawned {
        log::warn!("a session cannot start: {e}");
        send(Report::Refused {
            token,
            why: "The session did not start: see the log".into(),
        });
    }
}

/// Everything before the start, then the late baseline and the second look at the registry.
fn prepare(mut order: Order, send: &(dyn Fn(Report) + Sync)) {
    let token = order.token;
    let refuse = |why: String| send(Report::Refused { token, why });
    // Its TUI's `claude` gone first, then the directory and the registry (ADR 0165, part 4d-2).
    if let Some(report) = gone::held_for_tui(&order).or_else(|| held_back(&order)) {
        return send(report);
    }
    let resumed = matches!(order.kind, Kind::Resume(_));
    let id = match &order.kind {
        Kind::Resume(id) => id.clone(),
        Kind::New | Kind::Fork(_) => match identity::random_bytes() {
            Ok(bytes) => SessionId::from_random(bytes),
            Err(e) => return refuse(format!("The session did not start: {e}")),
        },
    };
    let found = match hosted::find_shell(order.runs.settings()) {
        Ok(found) => found,
        Err(e) => return refuse(format!("The session did not start: no shell is found: {e}")),
    };
    let state = order.state.as_deref();
    let (extra, notes) = match space_extra(order.space.as_ref(), &order.directory, state) {
        Ok(found) => found,
        Err(why) => return refuse(why),
    };
    // A space's gate once its checks passed (ADR 0176), before the line it changes.
    let gate = match space_gate::settle(&mut order) {
        Ok(gate) => gate,
        Err(why) => return refuse(why),
    };
    let launch = match launch_of(&order, &id, &found.executable, &extra, gate.is_some()) {
        Ok(launch) => launch,
        Err(why) => return refuse(why),
    };
    if let Err(why) = trust::settle(&mut order).and_then(|()| root_kept(&order)) {
        return refuse(why);
    }
    // The baseline before `claude` runs, so that the review shows what the session changed.
    let baseline = (order.trusted.clone())
        .filter(|_| order.baseline)
        .and_then(|root| Baseline::start(root, order.state.clone()));
    let in_time = baseline.as_ref().and_then(|b| b.wait(BASELINE_WAIT));
    let late = baseline.filter(|_| in_time.is_none());
    let Order {
        directory,
        project,
        trusted,
        runs,
        brief,
        ..
    } = order;
    if let Err(why) = still_gated(runs.native(), gate.as_ref(), trusted.as_ref()) {
        return refuse(why);
    }
    let set = set(&found.path, &id);
    let (buffer, started) =
        match start_runs(runs, &launch, &directory, project.as_deref(), set, brief) {
            Ok(started) => started,
            Err(why) => return refuse(why),
        };
    let child_started = child_started(&started);
    send(Report::Ready(Box::new(Ready {
        token,
        id: id.clone(),
        buffer,
        started,
        child_started,
        shell: found.path,
        passed_over: found.passed_over,
        directory,
        project,
        baseline: in_time.flatten(),
        notes,
    })));
    if let Some(Some(tree)) = late.and_then(|b| b.wait(BASELINE_GIVE_UP)) {
        send(Report::Baseline { token, tree });
    }
    if resumed {
        thread::sleep(RECHECK_AFTER);
        if live_count(&id) > 1 {
            send(Report::Twice { token });
        }
    }
}

/// A native session has no trust question of Claude Code's: oso's trust is its gate, looked at
/// again just before the spawn, after the baseline's wait (ADR 0165, Q5), or its space's
/// (ADR 0176).
///
/// # Errors
///
/// Why it does not start.
fn still_gated(
    native: bool,
    gate: Option<&space_gate::SpaceGate>,
    trusted: Option<&TrustedRoot>,
) -> Result<(), String> {
    match gate {
        Some(gate) if gate.revoked() => Err(space_gate::TAKEN_BACK.to_owned()),
        Some(_) => Ok(()),
        None if native && !trusted.is_some_and(|root| !root.revoked() && !root.replaced()) => {
            Err("The session did not start: its project is no longer trusted".to_owned())
        }
        None => Ok(()),
    }
}

/// A terminal's child's process id and start, read just after the spawn, while it runs as
/// the spawn left it, for a native resume's wait for it (ADR 0165, part 4d-2).
fn child_started(started: &Started) -> Option<(u32, std::time::SystemTime)> {
    let Started::Terminal(terminal) = started else {
        return None;
    };
    let pid = terminal.pid()?;
    let at = oso_platform::process::start_time(pid).ok().flatten()?;
    Some((pid, at))
}

/// The variables a session's terminal sets: a terminal's, and [`SESSION_VARIABLE`].
fn set(shell_path: &Path, id: &SessionId) -> Vec<(String, OsString)> {
    let mut set = shell::set(shell_path);
    set.push((SESSION_VARIABLE.to_owned(), id.as_str().into()));
    set
}

/// What an orchestrator's command line takes beyond any session's.
#[derive(Debug, Default)]
struct Extra {
    add_dirs: Vec<PathBuf>,
    system_prompt: Option<String>,
}

/// An orchestrator's directory checked again, just before it starts, and its `--add-dir`s and
/// prompt, with what the echo area says of a repository left out (ADR 0136, step 3). Nothing
/// for another session.
fn space_extra(
    space: Option<&SpaceStart>,
    directory: &Path,
    state: Option<&Path>,
) -> Result<(Extra, Vec<String>), String> {
    use crate::sessions::spaces::orchestrator;
    let Some(space) = space else {
        return Ok((Extra::default(), Vec::new()));
    };
    if space.in_space_directory {
        orchestrator::check_before_start(directory, space.made, &space.members)
            .map_err(|why| format!("The orchestrator did not start: {why}"))?;
    }
    let (kept, gone) = orchestrator::still_there(&space.trusted);
    let mut notes: Vec<String> = gone
        .iter()
        .map(|root| {
            format!(
                "{} is no longer the directory oso checked: left out",
                root.display()
            )
        })
        .collect();
    let instructions = match space
        .instructions
        .as_deref()
        .map(orchestrator::read_instructions)
    {
        Some(Ok(text)) => text,
        Some(Err(why)) => {
            notes.push(format!("The space's instructions were not read: {why}"));
            String::new()
        }
        None => String::new(),
    };
    let mut prompt = space.prompt.clone();
    if !gone.is_empty() {
        let roots: Vec<String> = gone.iter().map(|root| root.display().to_string()).collect();
        prompt.push_str(&format!(
            "\nNot added after all, as each is no longer the directory the space recorded: {}\n",
            orchestrator::plain(&roots.join(", "))
        ));
    }
    prompt.push_str(&orchestrator::instructions_section(&instructions));
    // A session in the space's directory that is not its orchestrator gets the checks alone.
    let mut add_dirs: Vec<PathBuf> = kept.into_iter().map(|member| member.root).collect();
    let (reads, left) = reads_checked(&space.reads, state);
    add_dirs.extend(reads);
    notes.extend(left);
    let extra = Extra {
        add_dirs,
        system_prompt: (!prompt.is_empty()).then_some(prompt),
    };
    Ok((extra, notes))
}

/// A worker's directories to read, each checked again in its repository as the tool's job did:
/// those that pass, and what the echo area says of each one left out.
fn reads_checked(
    reads: &[(crate::sessions::worktree::jobs::Trust, PathBuf)],
    state: Option<&Path>,
) -> (Vec<PathBuf>, Vec<String>) {
    let mut kept = Vec::new();
    let mut left = Vec::new();
    for (trust, dir) in reads {
        match crate::sessions::worktree::jobs::readable_in(trust, dir, state) {
            Ok(read) => kept.push(read.root),
            Err(why) => left.push(format!("Not given to read: {why}")),
        }
    }
    (kept, left)
}

/// Refuses a session whose trusted root was put in another's place since it was found: none
/// starts there.
fn root_kept(order: &Order) -> Result<(), String> {
    if order.trusted.as_ref().is_some_and(TrustedRoot::replaced) {
        return Err(
            "The session did not start: its project's root is another directory than the one \
             trusted; trust-project trusts the one there now, if it is yours"
                .to_owned(),
        );
    }
    Ok(())
}

/// The permission mode, model and effort a session's line passes: a native session's own, or
/// what a native session held for the TUI it is handed to (ADR 0165, part 4d), which is a
/// resume; none for any other terminal's start.
fn chosen_flags<'a>(
    kind: &Kind,
    runs: &'a Runs,
) -> (
    Option<crate::channel::protocol::mode::PermissionMode>,
    Option<&'a str>,
    Option<&'a str>,
) {
    match runs {
        Runs::Native(native) => (
            native.permission_mode,
            native.model.as_deref(),
            native.effort,
        ),
        Runs::Terminal(TerminalOrder {
            handed: Some(handed),
            ..
        }) if matches!(kind, Kind::Resume(_)) => (
            handed.permission_mode,
            handed.model.as_deref(),
            handed.effort,
        ),
        Runs::Terminal(_) => (None, None, None),
    }
}

/// The program and arguments that start the session `id` as `order` says, in `shell`: a
/// native one in a space's directory, `gated`, with the person's own settings alone
/// (ADR 0176).
fn launch_of(
    order: &Order,
    id: &SessionId,
    shell: &Executable,
    extra: &Extra,
    gated: bool,
) -> Result<Launch, String> {
    let claude = launch::configured(order.claude.as_deref())?;
    let oso = std::env::current_exe()
        .map_err(|e| format!("The session did not start: oso cannot find itself: {e}"))?;
    let start = match &order.kind {
        Kind::New => Start::New { id },
        Kind::Resume(_) => Start::Resume { id },
        Kind::Fork(from) => Start::Fork { from, id },
    };
    let settings = steer::settings(
        &oso,
        &steer::Parts {
            steer_worktrees: order.steer_worktrees,
        },
    )
    .map_err(|e| format!("The session did not start: {e}"))?;
    let (permission_mode, model, effort) = chosen_flags(&order.kind, &order.runs);
    let line = Line {
        start,
        name: &order.name,
        oso: &oso,
        oso_pid: std::process::id(),
        system_prompt: extra.system_prompt.as_deref(),
        brief: order.brief.as_deref(),
        settings: settings.as_deref(),
        add_dirs: &extra.add_dirs,
        native: order.runs.native(),
        user_settings_only: gated && order.runs.native(),
        permission_mode,
        model,
        effort,
    };
    let args = command::arguments(&line).map_err(|e| format!("The session did not start: {e}"))?;
    launch::launch(shell, claude.as_ref(), args, order.project.as_deref())
        .map_err(|e| format!("The session did not start: {e}"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    #[cfg(unix)]
    use std::time::Duration;

    use super::launch::inherit;
    use super::{SESSION_VARIABLE, set};
    use crate::sessions::identity::SessionId;

    #[test]
    fn a_session_passes_on_its_names_and_the_config_and_socket_directories_once_each() {
        let names = inherit(
            &["LANG".to_owned()],
            &["HTTPS_PROXY".to_owned(), "LANG".to_owned()],
        );
        assert_eq!(
            names,
            [
                "SSH_AUTH_SOCK",
                "LANG",
                "HTTPS_PROXY",
                "CLAUDE_CONFIG_DIR",
                "XDG_RUNTIME_DIR"
            ]
        );
        let named = inherit(&[], &["CLAUDE_CONFIG_DIR".to_owned()]);
        assert_eq!(
            named,
            ["SSH_AUTH_SOCK", "CLAUDE_CONFIG_DIR", "XDG_RUNTIME_DIR"]
        );
    }

    #[test]
    fn a_session_sets_a_terminals_variables_and_its_id() {
        let id = SessionId::from_random([7; 16]);
        let vars = set(Path::new("/bin/zsh"), &id);
        let get = |name: &str| {
            vars.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        assert_eq!(get("SHELL").as_deref(), Some("/bin/zsh"));
        assert_eq!(get("TERM_PROGRAM").as_deref(), Some("oso"));
        assert_eq!(get(SESSION_VARIABLE).as_deref(), Some(id.as_str()));
    }

    /// A terminal's order for `buffer`, a TUI handed `handed`.
    fn terminal_order(
        buffer: oso_core::buffer::BufferId,
        handed: Option<super::Chosen>,
    ) -> super::TerminalOrder {
        use oso_platform::process::pty::Size;
        use oso_term::{Palette, Rgb};
        super::TerminalOrder {
            buffer,
            settings: crate::terminal::shell::Settings::default(),
            env: Vec::new(),
            size: Size {
                rows: 6,
                columns: 40,
                width: 0,
                height: 0,
            },
            palette: Palette {
                foreground: Rgb::default(),
                background: Rgb::default(),
                cursor: None,
                ansi: [Rgb::default(); 16],
            },
            ready: Box::new(|| {}),
            handed,
        }
    }

    /// The TUI a native session is handed to passes what it held (ADR 0165, part 4d), and only
    /// as a resume; any other terminal's start passes none of them.
    #[test]
    fn only_the_tui_a_native_session_is_handed_to_takes_what_it_held() {
        use super::{Chosen, Kind, Runs, chosen_flags};
        use crate::channel::protocol::mode::PermissionMode;
        let mut editor = oso_core::editor::Editor::default();
        let buffer = editor.add_terminal_buffer("*claude: test*");
        let id = SessionId::from_random([7; 16]);
        let chosen = Chosen {
            permission_mode: Some(PermissionMode::AcceptEdits),
            model: Some("opus".to_owned()),
            effort: Some("high"),
        };
        let handed = Runs::Terminal(terminal_order(buffer, Some(chosen)));
        assert_eq!(
            chosen_flags(&Kind::Resume(id.clone()), &handed),
            (
                Some(PermissionMode::AcceptEdits),
                Some("opus"),
                Some("high")
            )
        );
        for kind in [Kind::New, Kind::Fork(id.clone())] {
            assert_eq!(chosen_flags(&kind, &handed), (None, None, None), "{kind:?}");
        }
        let plain = Runs::Terminal(terminal_order(buffer, None));
        for kind in [Kind::New, Kind::Resume(id.clone()), Kind::Fork(id)] {
            assert_eq!(chosen_flags(&kind, &plain), (None, None, None), "{kind:?}");
        }
    }

    /// A hosted session's start reports its `claude`'s process id: `Terminal::start` returns
    /// before the spawn, and a record with none took its own `claude` for another's.
    #[cfg(unix)]
    #[test]
    fn a_started_session_is_ready_with_its_childs_id() {
        use std::os::unix::fs::PermissionsExt;

        use super::{Kind, Order, Report, Runs, Started, spawn};

        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("oso-start-child-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A fresh private directory: one made by anyone else meanwhile fails the test.
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        // A stand-in `claude` that prints its id and waits for its terminal to hang up.
        let claude = dir.join("claude");
        std::fs::write(&claude, "#!/bin/sh\necho \"pid $$\"\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut editor = oso_core::editor::Editor::default();
        let buffer = editor.add_terminal_buffer("*claude: test*");
        let order = Order {
            token: 7,
            kind: Kind::New,
            name: "test".to_owned(),
            directory: dir.clone(),
            project: None,
            trusted: None,
            baseline: false,
            state: None,
            claude: Some(claude),
            runs: Runs::Terminal(terminal_order(buffer, None)),
            brief: None,
            steer_worktrees: false,
            check_shared: false,
            space: None,
        };
        let (tx, reports) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        spawn(order, move |report| {
            let _ = tx.lock().unwrap().send(report);
        });
        let ready = match reports.recv_timeout(Duration::from_secs(30)).unwrap() {
            Report::Ready(ready) => ready,
            other => panic!("not started: {other:?}"),
        };
        let Started::Terminal(terminal) = &ready.started else {
            panic!("not a terminal");
        };
        assert!(terminal.pid().is_some(), "no process id with the start");
        // Dropping the terminal hangs its own child up.
        drop(ready);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
