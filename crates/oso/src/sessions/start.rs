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
//! - A missing directory refuses before anything starts.
//! - In a trusted project, the baseline is taken first (ADR 0122), waiting at most
//!   [`BASELINE_WAIT`]; past that, the session starts and the baseline follows.
//! - The shell is found as ADR 0120 has it, and `claude` on `PATH` when the shell is not one
//!   that runs the wrapper, as [`launch`](super::launch) has it.
//! - The terminal starts here too, with ADR 0120's environment, `[sessions] env`'s names and
//!   `CLAUDE_CONFIG_DIR` and `OSO_SESSION`, so that the UI thread never waits on the spawn.
//! - A native session (ADR 0165) starts `claude -p` here instead, with the same variables and
//!   pipes for its stream, in a session of its own; [`stream`](super::stream) reads it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use oso_core::buffer::BufferId;
use oso_platform::process::pty::Size;
use oso_platform::process::{self, Executable};
use oso_term::{Palette, Terminal};

use super::command::{self, Line, SESSION_VARIABLE, Start};
use super::identity::{self, SessionId};
use super::launch::{self, Launch};
use super::registry;
use super::stream::{self, Stream};

mod runs;

use super::worktree::steer;
use crate::git::TrustedRoot;
use crate::review::{self, Reviewer};
use crate::terminal::hosted;
use crate::terminal::shell::{self, Settings};
use runs::start_runs;

/// The longest a session waits for its baseline before it starts (ADR 0122).
pub const BASELINE_WAIT: Duration = Duration::from_secs(2);

/// The longest a baseline is waited for at all, after the session started: git's own runs are
/// bounded, so this only ends a wait that went wrong.
const BASELINE_GIVE_UP: Duration = Duration::from_secs(60);

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
    /// The project's root, if the project is trusted to run git, for the baseline.
    pub trusted: Option<TrustedRoot>,
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
    /// `[terminal]`'s settings: the shell, `-l` and the names passed on.
    pub settings: Settings,
    /// `[sessions] env`: more names passed on from oso's environment.
    pub env: Vec<String>,
    /// Called from the stream's threads with what they read.
    pub out: stream::Deliver,
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
}

/// A session whose terminal started.
#[derive(Debug)]
pub struct Ready {
    pub token: u64,
    pub id: SessionId,
    pub buffer: BufferId,
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
fn prepare(order: Order, send: &(dyn Fn(Report) + Sync)) {
    let token = order.token;
    let refuse = |why: String| send(Report::Refused { token, why });
    if let Some(report) = held_back(&order) {
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
    let launch = match launch_of(&order, &id, &found.executable, &extra) {
        Ok(launch) => launch,
        Err(why) => return refuse(why),
    };
    if let Err(why) = root_kept(&order) {
        return refuse(why);
    }
    // The baseline before `claude` runs, so that the review shows what the session changed.
    let baseline = order
        .trusted
        .clone()
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
    // A native session has no trust question of Claude Code's: oso's trust is its gate, looked
    // at again now, after the baseline's wait (ADR 0165, Q5).
    if runs.native()
        && !trusted
            .as_ref()
            .is_some_and(|root| !root.revoked() && !root.replaced())
    {
        return refuse("The session did not start: its project is no longer trusted".to_owned());
    }
    let set = set(&found.path, &id);
    let (buffer, started) =
        match start_runs(runs, &launch, &directory, project.as_deref(), set, brief) {
            Ok(started) => started,
            Err(why) => return refuse(why),
        };
    send(Report::Ready(Box::new(Ready {
        token,
        id: id.clone(),
        buffer,
        started,
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

/// What stops `order` before anything is made: a missing directory, a new session's directory
/// where another live one runs when it is to ask first, or a resume whose conversation is live.
fn held_back(order: &Order) -> Option<Report> {
    let token = order.token;
    if !order.directory.is_dir() {
        return Some(Report::Refused {
            token,
            why: "The session's directory is not there any more".into(),
        });
    }
    match &order.kind {
        Kind::Resume(id) => live(id).map(|(pid, directory)| Report::Live {
            token,
            pid,
            directory,
        }),
        Kind::New | Kind::Fork(_) => {
            (order.check_shared && runs_in(&order.directory)).then_some(Report::Shared { token })
        }
    }
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

/// The program and arguments that start the session `id` as `order` says, in `shell`.
fn launch_of(
    order: &Order,
    id: &SessionId,
    shell: &Executable,
    extra: &Extra,
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
    };
    let args = command::arguments(&line).map_err(|e| format!("The session did not start: {e}"))?;
    launch::launch(shell, claude.as_ref(), args, order.project.as_deref())
        .map_err(|e| format!("The session did not start: {e}"))
}

/// A live process of the session `id`, by the registry: its pid and directory. With no pid,
/// the registry could not tell: its directory is not known, or it holds more entries than are
/// read. A resume is then refused too, as the safe side.
fn live(id: &SessionId) -> Option<(Option<u32>, Option<PathBuf>)> {
    let Some(directory) = registry::directory() else {
        return Some((None, None));
    };
    let listing = registry::list(&directory);
    let live = registry::live_with(&listing.entries, id, process::start_time);
    match live.first() {
        Some(entry) => Some((Some(entry.pid), entry.cwd.clone())),
        None => listing.cut.then_some((None, None)),
    }
}

/// Whether the registry has a live session whose directory is `directory`.
fn runs_in(directory: &Path) -> bool {
    let Some(dir) = registry::directory() else {
        return false;
    };
    registry::read(&dir).iter().any(|entry| {
        entry
            .cwd
            .as_deref()
            .is_some_and(|cwd| oso_core::project::same_place(cwd, directory))
            && registry::liveness(entry, process::start_time) != registry::Live::No
    })
}

/// How many live processes the registry has for `id`.
fn live_count(id: &SessionId) -> usize {
    let Some(directory) = registry::directory() else {
        return 0;
    };
    let entries = registry::read(&directory);
    registry::live_with(&entries, id, process::start_time).len()
}

/// A baseline being taken by a review's thread of its own.
struct Baseline {
    _reviewer: Reviewer,
    trees: mpsc::Receiver<Option<String>>,
}

impl Baseline {
    fn start(root: TrustedRoot, state: Option<PathBuf>) -> Option<Self> {
        let (sender, trees) = mpsc::channel();
        let reviewer = Reviewer::start(root, state, move |report| {
            let tree = match report {
                review::Report::Baseline { tree, .. } => Some(tree.as_str().to_owned()),
                review::Report::Failed { why, .. } => {
                    log::warn!("a session's baseline was not taken: {why}");
                    None
                }
                _ => return true,
            };
            let _ = sender.send(tree);
            false
        })
        .map_err(|e| log::warn!("a session's baseline cannot be taken: {e}"))
        .ok()?;
        reviewer.ask(review::Request::Baseline).then_some(Self {
            _reviewer: reviewer,
            trees,
        })
    }

    /// The tree, `Some(None)` if it failed, or `None` if it did not come within `wait`.
    fn wait(&self, wait: Duration) -> Option<Option<String>> {
        match self.trees.recv_timeout(wait) {
            Ok(tree) => Some(tree),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => Some(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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
}
