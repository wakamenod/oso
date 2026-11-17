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
use super::registry::{self, Live, PROC_START_SLACK, is_plain_directory};
use super::spaces::Member;
use super::spaces::probe::main_checkout;
use super::spaces::stored::MAX_MEMBERS;
use super::tools::{quoted, quoted_within};

/// The longest a path is shown behind `?`, in characters, so that its middle is seen too.
const WHOLE: usize = 1024;

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
    /// The parent's name as the system gives it, already made safe to show.
    pub program: Option<String>,
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
    /// Its process's name as the system gives it, safe to show: any process of the person's
    /// can write a registry entry, so the question names what really runs.
    pub program: Option<String>,
    /// The registry's name, not cleaned.
    pub name: Option<String>,
    /// The directory it runs in, its own real path.
    pub directory: PathBuf,
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
    // A run with `-p` has no one to type `/exit` and nothing to move.
    if entry
        .kind
        .as_deref()
        .is_some_and(|kind| kind != "interactive")
    {
        return Err("only an interactive Claude Code session may move".into());
    }
    // The entry is the parent's as it was when the connection came, and the process with its
    // id now is that one still.
    let then = |pid: u32| Ok((pid == look.pid).then_some(look.started));
    if registry::liveness(&entry, then) != Live::Yes {
        return Err(NOT_FOUND.into());
    }
    if !matches!(started(look.pid), Ok(Some(now)) if same_start(now, look.started)) {
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
        program: look.program.clone(),
        name: entry.name,
        directory,
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
        Ok(Some(now)) => same_start(now, then),
        Ok(None) => false,
        Err(_) => true,
    }
}

/// Whether a process that started at `now` is the one that started at `then`. Linux counts a
/// start from when the system booted, which moves when the clock is set, so a start within
/// the registry's slack is the same: a reused id cannot start that close to a process that
/// lived through a connection, a question and a `y`.
fn same_start(now: SystemTime, then: SystemTime) -> bool {
    let apart = now
        .duration_since(then)
        .or_else(|_| then.duration_since(now));
    apart.is_ok_and(|apart| apart <= PROC_START_SLACK)
}

/// The session of oso's record with the asker's conversation id, as [`judge`] needs it.
#[derive(Debug, Clone, Copy)]
pub struct Recorded<'a> {
    pub name: &'a str,
    pub runs: bool,
    pub directory: &'a Path,
    /// It is a space's orchestrator, which runs in that space's own directory.
    pub orchestrates: bool,
}

/// What oso holds that decides a move, as [`judge`] needs it.
#[derive(Debug, Clone, Copy)]
pub struct Held<'a> {
    /// The session of oso's record with that conversation, if any.
    pub record: Option<Recorded<'a>>,
    /// oso trusts the repository, by its path and identity.
    pub trusted: bool,
    /// The space has the repository already.
    pub member: bool,
    /// How many repositories the space has.
    pub members: usize,
}

/// Whether `asker` may move into the space `space`, as `held` stands: whether the space takes
/// its repository then, or why it may not, for the agent.
///
/// # Errors
///
/// Why it may not move.
pub fn judge(asker: &Asker, space: &str, held: &Held<'_>) -> Result<bool, String> {
    if let Some(record) = held.record {
        if record.runs {
            return Err(format!(
                "oso runs this conversation already, as session {}",
                quoted(record.name)
            ));
        }
        if record.orchestrates {
            return Err("this conversation is a space's orchestrator in oso's record".into());
        }
        if record.directory != asker.directory {
            return Err(
                "oso's record holds this conversation in another directory: stop it \
                 here and the person resumes it in oso"
                    .into(),
            );
        }
    }
    if !held.trusted {
        return Err(format!(
            "oso does not trust repository {}: the person trusts it with trust-project, then \
             ask again",
            quoted(&asker.main.root.display().to_string())
        ));
    }
    if !held.member && held.members >= MAX_MEMBERS {
        return Err(format!(
            "space {} holds {MAX_MEMBERS} repositories, the most it may",
            quoted(space)
        ));
    }
    Ok(!held.member)
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
        "move session {} ({}, pid {}, in {}, outside oso) into space {}{adding}? It stops \
         there first, then resumes here with your settings and may start sessions as yours do.",
        quoted(name),
        program(asker),
        asker.pid,
        quoted(&asker.directory.display().to_string()),
        quoted(space),
    )
}

/// The asker's process as the question names it.
fn program(asker: &Asker) -> String {
    asker
        .program
        .as_deref()
        .map_or_else(|| "a process with no name".to_owned(), quoted)
}

/// What `?` shows for the question `label`, with the space that holds the worktree now, if
/// another does: the move takes it from that space.
pub fn details(label: &str, asker: &Asker, space: &str, held_by: Option<&str>) -> String {
    let whole = |path: &Path| quoted_within(&path.display().to_string(), WHOLE);
    let worktree = asker
        .worktree
        .as_deref()
        .map_or_else(String::new, |worktree| {
            let leaves = held_by.map_or_else(String::new, |other| {
                format!(", which leaves space {}", quoted(other))
            });
            format!("  Worktree:     {}{leaves}\n", whole(worktree))
        });
    format!(
        "A Claude Code session outside oso asks, as its registry entry says:\n\n  {label}\n\ny \
         allows it, n or C-g refuses it.\n\n  Conversation: {}\n  Process:      {} {}\n  \
         Directory:    {}\n{worktree}  Repository:   {}\n  Space:        {}\n\nOn y, the \
         session is told to end its turn and the person to type /exit where it runs. Once that \
         process has ended, oso resumes the conversation in that directory, in this space, as \
         started on your y, with your settings and none of the flags it ran with. Background \
         work in the old process ends with it. Killing the buffer *moving: …* cancels the move \
         while oso waits.\n",
        asker.id.as_str(),
        asker.pid,
        program(asker),
        whole(&asker.directory),
        whole(&asker.main.root),
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

/// What the echo area says when `space` closed, or lost the repository, while oso waited.
pub fn space_gone(name: &str, space: &str) -> String {
    format!(
        "Space {} closed or left the repository while oso waited, so {} was not resumed: adopt \
         it with session-adopt",
        quoted(space),
        quoted(name)
    )
}

/// What the echo area says when oso stopped trusting the repository while it waited.
pub fn untrusted_meanwhile(name: &str) -> String {
    format!(
        "oso stopped trusting the repository of {} while it waited, so it was not resumed",
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
