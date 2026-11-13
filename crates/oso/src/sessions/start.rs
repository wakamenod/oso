//! Getting a hosted session ready to start, on a short `oso-session-start` thread of its own
//! (ADR 0121, "A session" and "Restore and the fork hazard"): everything that touches the file
//! system or the process table before `claude` runs, so the UI thread never waits on it.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; the caller gives an [`Order`] and
//! takes [`Report`]s through the function it gives.
//!
//! - A new session's id is made from 16 bytes of `/dev/urandom`; a fork gets a new one too.
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
use crate::review::{self, Reviewer, TrustedRoot};
use crate::terminal::hosted;
use crate::terminal::shell::{self, Settings};

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
    /// The terminal it runs in.
    pub terminal: TerminalOrder,
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
    pub terminal: Terminal,
    /// The shell's path and the shells passed over before it, for `*Messages*`.
    pub shell: PathBuf,
    pub passed_over: Vec<String>,
    pub directory: PathBuf,
    /// The project it runs in, as the order gave it.
    pub project: Option<PathBuf>,
    /// Its baseline's tree id, if it was taken in time.
    pub baseline: Option<String>,
}

/// What the thread found.
#[derive(Debug)]
pub enum Report {
    /// Start it.
    Ready(Ready),
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
    if !order.directory.is_dir() {
        return refuse("The session's directory is not there any more".into());
    }
    let resumed = match &order.kind {
        Kind::Resume(id) => Some(id.clone()),
        Kind::New | Kind::Fork(_) => None,
    };
    if let Some(id) = &resumed
        && let Some((pid, directory)) = live(id)
    {
        return send(Report::Live {
            token,
            pid,
            directory,
        });
    }
    let id = match &order.kind {
        Kind::Resume(id) => id.clone(),
        Kind::New | Kind::Fork(_) => match identity::random_bytes() {
            Ok(bytes) => SessionId::from_random(bytes),
            Err(e) => return refuse(format!("The session did not start: {e}")),
        },
    };
    let found = match hosted::find_shell(&order.terminal.settings) {
        Ok(found) => found,
        Err(e) => return refuse(format!("The session did not start: no shell is found: {e}")),
    };
    let launch = match launch_of(&order, &id, &found.executable) {
        Ok(launch) => launch,
        Err(why) => return refuse(why),
    };
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
        terminal: given,
        ..
    } = order;
    let start = oso_term::Start {
        executable: launch.program,
        args: launch.args,
        cwd: Some(directory.clone()),
        project: project.clone(),
        inherit: inherit(&given.settings.env, &given.env),
        set: set(&found.path, &id),
        size: given.size,
        scrollback: given.settings.scrollback,
        palette: given.palette,
    };
    let terminal = match Terminal::start(start, given.ready) {
        Ok(terminal) => terminal,
        Err(e) => return refuse(format!("The session's terminal did not start: {e}")),
    };
    send(Report::Ready(Ready {
        token,
        id: id.clone(),
        buffer: given.buffer,
        terminal,
        shell: found.path,
        passed_over: found.passed_over,
        directory,
        project,
        baseline: in_time.flatten(),
    }));
    if let Some(Some(tree)) = late.and_then(|b| b.wait(BASELINE_GIVE_UP)) {
        send(Report::Baseline { token, tree });
    }
    if resumed.is_some() {
        thread::sleep(RECHECK_AFTER);
        if live_count(&id) > 1 {
            send(Report::Twice { token });
        }
    }
}

/// The names a session's terminal passes on: a terminal's, `[sessions] env`'s, and
/// `CLAUDE_CONFIG_DIR`, so that the session's registry is the one oso reads.
fn inherit(terminal: &[String], sessions: &[String]) -> Vec<String> {
    let mut names = shell::inherit(terminal);
    for name in sessions
        .iter()
        .map(String::as_str)
        .chain([registry::CONFIG_VARIABLE])
    {
        if !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// The variables a session's terminal sets: a terminal's, and [`SESSION_VARIABLE`].
fn set(shell_path: &Path, id: &SessionId) -> Vec<(String, OsString)> {
    let mut set = shell::set(shell_path);
    set.push((SESSION_VARIABLE.to_owned(), id.as_str().into()));
    set
}

/// The program and arguments that start the session `id` as `order` says, in `shell`.
fn launch_of(order: &Order, id: &SessionId, shell: &Executable) -> Result<Launch, String> {
    let claude = match &order.claude {
        Some(path) => Some(
            Executable::absolute(path)
                .map_err(|_| "[sessions] claude is not an executable file".to_owned())?,
        ),
        None => None,
    };
    let oso = std::env::current_exe()
        .map_err(|e| format!("The session did not start: oso cannot find itself: {e}"))?;
    let start = match &order.kind {
        Kind::New => Start::New { id },
        Kind::Resume(_) => Start::Resume { id },
        Kind::Fork(from) => Start::Fork { from, id },
    };
    let line = Line {
        start,
        name: &order.name,
        oso: &oso,
        oso_pid: std::process::id(),
        system_prompt: None,
        brief: None,
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

    use super::{SESSION_VARIABLE, inherit, set};
    use crate::sessions::identity::SessionId;

    #[test]
    fn a_session_passes_on_a_terminals_names_its_own_and_the_config_directory_once_each() {
        let names = inherit(
            &["LANG".to_owned()],
            &["HTTPS_PROXY".to_owned(), "LANG".to_owned()],
        );
        assert_eq!(
            names,
            ["SSH_AUTH_SOCK", "LANG", "HTTPS_PROXY", "CLAUDE_CONFIG_DIR"]
        );
        let named = inherit(&[], &["CLAUDE_CONFIG_DIR".to_owned()]);
        assert_eq!(named, ["SSH_AUTH_SOCK", "CLAUDE_CONFIG_DIR"]);
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
