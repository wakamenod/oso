//! Moving a session started outside oso into a space, at its request (ADR 0151): who asks,
//! known by the registry entry of its bridge's parent; where it runs and its repository; the
//! wait for the outside process to end; and what the person and the agent are told.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread. The look and the wait touch the
//! disk and the process table, so each runs on a thread of its own and gives what it found
//! through the function the caller gives.
//!
//! - The agent names only a space. The conversation, its directory and its process come from
//!   the registry entry of the bridge's parent, checked against when that parent started.
//! - A linked worktree counts only as ADR 0114 counts one: its main repository is trusted and
//!   records it back. The repository is a main checkout, as `space-add-repo` takes one.
//! - Registry text in the question is quoted and escaped, as every agent question's is.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use oso_platform::process;

use super::identity::SessionId;
use super::registry::{self, Live, is_plain_directory};
use super::spaces::Member;
use super::spaces::probe::main_checkout;
use super::tools::quoted;

/// How often the wait looks whether the outside process has ended.
pub const POLL: Duration = Duration::from_millis(500);

/// How long the wait goes before the echo area asks the person to stop it by hand.
pub const LATE: Duration = Duration::from_secs(120);

/// How long the wait goes before it gives up.
pub const GIVE_UP: Duration = Duration::from_secs(300);

/// What refuses a call whose session the registry does not hold.
pub const NOT_FOUND: &str = "oso cannot find this session in Claude Code's registry: stop it \
     here and ask the person to adopt it with session-adopt";

/// What the agent is told once the person agreed: it ends its turn, and the person stops it.
pub const REPLY: &str = "The person agreed. End your turn now, calling no more tools, with one \
     short line telling the person to type /exit here; once this process has ended, oso resumes \
     this conversation in the space. Do not resume it anywhere yourself.";

/// What the look is given: the bridge's parent and when it started, as the system said when
/// the connection came, and what the look checks the directory against.
#[derive(Debug, Clone)]
pub struct Look {
    pub pid: u32,
    pub started: SystemTime,
    /// The roots of trusted projects, which a linked worktree's claim must name.
    pub trusted: Arc<[PathBuf]>,
    /// The home directory, which is never a project.
    pub home: Option<PathBuf>,
}

/// The session that asks, as the look found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asker {
    /// Its process, which the wait watches.
    pub pid: u32,
    pub started: SystemTime,
    pub id: SessionId,
    /// The registry's name, not cleaned.
    pub name: Option<String>,
    /// The directory it runs in, its own real path.
    pub directory: PathBuf,
    /// The registry says it is busy.
    pub busy: bool,
    /// Its repository's main checkout.
    pub main: Member,
    /// The linked worktree it runs in, if it runs in one.
    pub worktree: Option<PathBuf>,
}

/// Finds the session whose bridge's parent `look` names, in the registry at `dir`, with
/// `started` giving when a process started, as `oso_platform::process::start_time` does.
///
/// # Errors
///
/// Why it cannot move, for the agent.
pub fn look(
    dir: Option<&Path>,
    look: &Look,
    started: impl Fn(u32) -> io::Result<Option<SystemTime>>,
) -> Result<Asker, String> {
    let dir = dir.ok_or(NOT_FOUND)?;
    let name = OsString::from(format!("{}.json", look.pid));
    let entry = registry::read_file(dir, &name).ok_or(NOT_FOUND)?;
    let id = entry.session_id.clone().ok_or(NOT_FOUND)?;
    // The entry is the parent's as it was when the connection came, and the process with its
    // id now is that one still.
    let then = |pid: u32| Ok((pid == look.pid).then_some(look.started));
    if registry::liveness(&entry, then) != Live::Yes {
        return Err(NOT_FOUND.into());
    }
    if !matches!(started(look.pid), Ok(Some(now)) if now == look.started) {
        return Err("this session's process is not the one that connected".into());
    }
    let directory = entry.cwd.clone().ok_or(NOT_FOUND)?;
    if !is_plain_directory(&directory) {
        return Err("Claude Code's registry gives this session a directory oso cannot use".into());
    }
    match std::fs::canonicalize(&directory) {
        Ok(real) if real == directory => {}
        Ok(_) => return Err("this session's directory is reached through a link".into()),
        Err(_) => return Err("this session's directory is gone".into()),
    }
    let (main, worktree) = repository(&directory, look)?;
    Ok(Asker {
        pid: look.pid,
        started: look.started,
        id,
        name: entry.name,
        directory,
        busy: entry.status.as_deref() == Some("busy"),
        main,
        worktree,
    })
}

/// The main checkout of the repository `directory` is in, and the linked worktree it is in,
/// if it is in one, as ADR 0114 finds a project and checks a worktree's claim.
fn repository(directory: &Path, look: &Look) -> Result<(Member, Option<PathBuf>), String> {
    // The walk starts at the directory of what it is given, which need not exist.
    let found = crate::project::detect(
        &directory.join(".oso-move"),
        look.home.as_deref(),
        &look.trusted,
    )
    .ok_or("this session's directory is in no repository")?;
    let (main, worktree) = match (&found.worktree_of, found.checked_main) {
        (None, _) => (found.root.path, None),
        (Some(_), Some(main)) => (main.path, Some(found.root.path)),
        (Some(_), None) => {
            return Err(
                "this session runs in a worktree of a repository oso does not trust: \
                 the person trusts it with trust-project, then ask again"
                    .into(),
            );
        }
    };
    Ok((main_checkout(&main)?, worktree))
}

/// What the wait does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The process still runs.
    Wait,
    /// It still runs at [`LATE`]: the person is asked to stop it, once.
    Late,
    /// It has ended.
    Stopped,
    /// It still runs at [`GIVE_UP`].
    GiveUp,
}

/// The wait's next step after `waited`, the process `running` or not, and the person told it
/// was late already or not.
pub fn step(waited: Duration, running: bool, told_late: bool) -> Step {
    if !running {
        Step::Stopped
    } else if waited >= GIVE_UP {
        Step::GiveUp
    } else if waited >= LATE && !told_late {
        Step::Late
    } else {
        Step::Wait
    }
}

/// Whether the process `pid`, which started at `then`, still runs, by `started`. One the
/// system cannot tell of counts as running: a resume then would fork the conversation.
pub fn running(
    pid: u32,
    then: SystemTime,
    started: impl Fn(u32) -> io::Result<Option<SystemTime>>,
) -> bool {
    match started(pid) {
        Ok(Some(now)) => now == then,
        Ok(None) => false,
        Err(_) => true,
    }
}

/// Looks on a thread of its own, `oso-session-move`, and calls `send` with what it found.
///
/// # Errors
///
/// If no thread starts.
pub fn spawn_look(
    look: Look,
    send: impl FnOnce(Result<Asker, String>) + Send + 'static,
) -> io::Result<()> {
    thread::Builder::new()
        .name("oso-session-move".into())
        .spawn(move || {
            let dir = registry::directory();
            send(self::look(dir.as_deref(), &look, process::start_time));
        })
        .map(drop)
}

/// Watches the process `pid`, which started at `then`, every [`POLL`] on a thread of its own,
/// `oso-session-move-wait`, and calls `send` with each step but [`Step::Wait`], until it has
/// stopped, the wait gave up, `stop` is set or `send` returns false.
///
/// # Errors
///
/// If no thread starts.
pub fn spawn_wait(
    pid: u32,
    then: SystemTime,
    stop: Arc<AtomicBool>,
    send: impl Fn(Step) -> bool + Send + 'static,
) -> io::Result<()> {
    thread::Builder::new()
        .name("oso-session-move-wait".into())
        .spawn(move || {
            let began = Instant::now();
            let mut told_late = false;
            loop {
                thread::sleep(POLL);
                if stop.load(Ordering::Acquire) {
                    return;
                }
                let alive = running(pid, then, process::start_time);
                let next = step(began.elapsed(), alive, told_late);
                told_late |= next == Step::Late;
                if next != Step::Wait && !send(next) {
                    return;
                }
                if matches!(next, Step::Stopped | Step::GiveUp) {
                    return;
                }
            }
        })
        .map(drop)
}

/// What a question and the echo area call the session: its registry name, else its id's
/// first 8 digits.
pub fn called(name: Option<&str>, id: &SessionId) -> String {
    name.filter(|name| !name.trim().is_empty())
        .map_or_else(|| id.as_str().chars().take(8).collect(), str::to_owned)
}

/// The question: `asker`, named `name`, moves into `space`, adding repository `adds` to it if
/// it is not a member yet.
pub fn question(name: &str, asker: &Asker, space: &str, adds: Option<&Path>) -> String {
    let adding = adds.map_or_else(String::new, |root| {
        format!(
            ", adding repository {} to it",
            quoted(&root.display().to_string())
        )
    });
    format!(
        "move session {} (claude, pid {}, in {}, outside oso) into space {}{adding}? It stops \
         there first, then resumes here with your settings and may start sessions as yours do.",
        quoted(name),
        asker.pid,
        quoted(&asker.directory.display().to_string()),
        quoted(space),
    )
}

/// What `?` shows for the question `label`.
pub fn details(label: &str, asker: &Asker, space: &str) -> String {
    let worktree = asker
        .worktree
        .as_ref()
        .map_or_else(String::new, |worktree| {
            format!(
                "  Worktree:     {}\n",
                quoted(&worktree.display().to_string())
            )
        });
    format!(
        "A Claude Code session outside oso asks:\n\n  {label}\n\ny allows it, n or C-g refuses \
         it.\n\n  Conversation: {}\n  Process:      {}\n  Directory:    {}\n{worktree}  \
         Repository:   {}\n  Space:        {}\n\nOn y, the session is told to end its turn and \
         the person to type /exit where it runs. Once that process has ended, oso resumes the \
         conversation in that directory, in this space, with your settings and none of the \
         flags it ran with. Background work in the old process ends with it. Killing the \
         buffer *moving: …* cancels the move while oso waits.\n",
        asker.id.as_str(),
        asker.pid,
        quoted(&asker.directory.display().to_string()),
        quoted(&asker.main.root.display().to_string()),
        quoted(space),
    )
}

/// What the waiting buffer and the echo area say while oso waits for `name` to stop.
pub fn waiting(name: &str, space: &str) -> String {
    format!(
        "Moving {} into space {}: type /exit where it runs; oso resumes it here once it stops \
         (kill this buffer to cancel)",
        quoted(name),
        quoted(space)
    )
}

/// What the echo area says at [`LATE`].
pub fn late(name: &str) -> String {
    format!(
        "{} still runs outside oso: type /exit there; oso resumes it once it stops",
        quoted(name)
    )
}

/// What the echo area says when the wait gave up.
pub fn gave_up(name: &str) -> String {
    format!(
        "{} did not stop outside oso, so nothing was resumed: stop it there and adopt it with \
         session-adopt",
        quoted(name)
    )
}

#[cfg(test)]
mod tests;
