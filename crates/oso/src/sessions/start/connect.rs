//! A native session's start that Remote Control may connect (ADR 0181): what is sent of its
//! repository as it connects, read by git, and whether it may be; and that it is where it was,
//! for a session connected again by `oso-restart`.
//!
//! Must not depend on: `app`. [`look`] and [`settle`] run git and read the disk, so they run on
//! threads of their own: a start thread, and the one before the person's question.
//!
//! - A start is [`Leave::Blocked`] unless the app gives it a [`Connect`], which is made for one
//!   start and cannot be copied: at the person's `y`, from the parent's restart state, or as
//!   Claude Code's settings connect each session at start (`at_startup`, ADR 0189), which are
//!   read again here: no longer on, it starts blocked and says nothing.
//! - Judged just before the line is built, under the trust the start was judged by, or in a
//!   space's own directory under its gate, which must be in no repository. Refused, it starts
//!   blocked, and says why.
//! - Every address the repository's configuration names is read, what a rewrite makes of it too,
//!   and each must pass (`address`). A git that does not answer refuses, and so does a
//!   directory below another's repository, which git here is kept from and `claude` may find.
//! - What a refusal says names no address and no remote. Between this look and the spawn, and as
//!   long as the session runs, what its own tools make of the configuration is not seen.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::space_gate::SpaceGate;
use super::{Order, Runs};
use crate::git::{Git, TrustedRoot, empty_hooks, identity, printable};
use crate::sessions::native::wire::remote::RemoteId;

pub mod address;
pub mod at_startup;

/// The most bytes of the configuration's remotes read.
const MAX_CONFIG: usize = 256 * 1024;

/// The most addresses sent: the question shows each, whole.
const MAX_SHOWN: usize = 12;

/// The most characters of an address or a branch sent, as it is shown.
const MAX_CELLS: usize = 400;

/// The configuration's keys an address, or a remote's name that may be one, is in.
const KEYS: &str =
    r"^(remote\..*\.(url|pushurl)|remote\.pushdefault|branch\..*\.(remote|pushremote))$";

/// Where git before its configuration kept remotes.
const OLD: [&str; 2] = ["remotes", "branches"];

const NO_ANSWER: &str = "git did not say what the repository's remotes are";
const MOVED: &str = "its directory or its repository is not the one it was connected in";
const CHANGED: &str = "its git remotes or its branch changed since the question";
const TOO_MANY: &str = "its repository has more git remotes than the question can show";
const NO_TRUST: &str = "its project is not trusted now";
const NO_STATE: &str = "oso has no state directory, which its git needs";
const IN_REPOSITORY: &str = "its directory is in another repository";

/// Whether a native start's Remote Control is blocked.
#[derive(Debug, Default)]
pub enum Leave {
    /// The line blocks it: every start but the two a [`Connect`] is made for.
    #[default]
    Blocked,
    /// The line does not.
    Connect(Connect),
}

/// Leave for one start not to block Remote Control.
#[derive(Debug)]
pub struct Connect {
    /// What it must find as it was.
    was: Was,
}

/// What a start with leave to connect must find as it was.
#[derive(Debug)]
enum Was {
    /// What the person's question said is sent.
    Shown(Sent),
    /// Where the session was connected before `oso-restart`.
    Back(Back),
    /// Nothing: Claude Code's settings connect each session at start, as these files say.
    AtStartup(Box<at_startup::Files>),
}

/// A connection to come back to.
#[derive(Debug)]
pub struct Back {
    /// The session's id there, which the answer must have.
    pub id: RemoteId,
    /// Where it ran.
    pub place: Place,
}

/// Where a session that connects runs: its directory and its repository's, by device and inode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub directory: (u64, u64),
    pub repository: Option<(u64, u64)>,
}

/// What is sent of a session's repository, as it is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sent {
    pub addresses: Vec<String>,
    pub branch: Option<String>,
}

/// How a native start's Remote Control was settled, for the app.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum Settled {
    #[default]
    Blocked,
    /// It was to connect, and started blocked: why.
    Refused(String),
    /// It was to connect as Claude Code's settings said, which say so no longer, here: blocked,
    /// with nothing to say.
    NotAtStartup,
    /// Not blocked: where it runs, where the system says so.
    Connects(Option<Place>),
}

impl Connect {
    /// Leave at the person's `y` to a question that said `sent`.
    pub fn asked(sent: Sent) -> Self {
        Self {
            was: Was::Shown(sent),
        }
    }

    /// Leave to come back to `back`, which the parent's restart state carried.
    pub fn again(back: Back) -> Self {
        Self {
            was: Was::Back(back),
        }
    }

    /// Leave as Claude Code's settings connect each session at start, which `files` are read
    /// again for (ADR 0189).
    pub fn at_startup(files: at_startup::Files) -> Self {
        Self {
            was: Was::AtStartup(Box::new(files)),
        }
    }

    /// The session's id there, if it comes back.
    pub fn back(&self) -> Option<&RemoteId> {
        match &self.was {
            Was::Shown(_) | Was::AtStartup(_) => None,
            Was::Back(back) => Some(&back.id),
        }
    }
}

impl Leave {
    /// Whether the line blocks it.
    pub fn blocked(&self) -> bool {
        matches!(self, Self::Blocked)
    }

    /// Whether it is the leave of Claude Code's settings (ADR 0189).
    pub fn by_settings(&self) -> bool {
        matches!(
            self,
            Self::Connect(Connect {
                was: Was::AtStartup(_)
            })
        )
    }
}

/// Settles native `order`'s Remote Control, its trust or its space's `gate` judged already:
/// blocked unless every check passes.
pub(super) fn settle(order: &mut Order, gate: Option<&SpaceGate>) -> Settled {
    let Runs::Native(native) = &mut order.runs else {
        return Settled::Blocked;
    };
    let Leave::Connect(connect) = std::mem::take(&mut native.remote) else {
        return Settled::Blocked;
    };
    if let Was::AtStartup(files) = &connect.was {
        match still_at_startup(files, &order.directory) {
            Ok(true) => {}
            Ok(false) => return Settled::NotAtStartup,
            Err(why) => return Settled::Refused(why),
        }
    }
    let judged = match (&order.trusted, gate) {
        (Some(root), _) => judge(root, &order.directory, order.state.as_deref(), &connect),
        (None, Some(gate)) => (look_space(gate, &order.directory))
            .and_then(|place| judge_place(place, &Sent::default(), &connect)),
        (None, None) => Err(NO_TRUST.to_owned()),
    };
    match judged {
        Ok(place) => {
            native.remote = Leave::Connect(connect);
            Settled::Connects(place)
        }
        Err(why) => Settled::Refused(why),
    }
}

/// Whether `connect` holds in `directory` under `root`: where that is.
fn judge(
    root: &TrustedRoot,
    directory: &Path,
    state: Option<&Path>,
    connect: &Connect,
) -> Result<Option<Place>, String> {
    let (sent, place) = look(root, directory, state)?;
    judge_place(place, &sent, connect)
}

/// Whether `connect` holds where a look found `sent`, at `place`.
fn judge_place(
    place: Option<Place>,
    sent: &Sent,
    connect: &Connect,
) -> Result<Option<Place>, String> {
    match &connect.was {
        Was::Shown(shown) if shown != sent => Err(CHANGED.to_owned()),
        Was::Back(back) if place != Some(back.place) => Err(MOVED.to_owned()),
        Was::Shown(_) | Was::Back(_) | Was::AtStartup(_) => Ok(place),
    }
}

/// Whether Claude Code's settings, as `files` and `directory`'s project say now, still connect a
/// session as it starts.
fn still_at_startup(files: &at_startup::Files, directory: &Path) -> Result<bool, String> {
    match files.read() {
        at_startup::Reading::On => {}
        at_startup::Reading::Off => return Ok(false),
        at_startup::Reading::Unread(why) => return Err(why),
    }
    at_startup::project_off(directory).map(|off| !off)
}

/// What is sent of a space's own `directory` as a session there connects, for the person's
/// question: nothing of git's, as it must be in no repository, nor below one (ADR 0189).
///
/// # Errors
///
/// Why a session there does not connect.
pub fn look_space_directory(directory: &Path) -> Result<Sent, String> {
    match in_repository(directory) {
        true => Err(IN_REPOSITORY.to_owned()),
        false => Ok(Sent::default()),
    }
}

/// Where a session in a space's own `directory`, under its `gate`, runs: a directory in no
/// repository, nor below one, so that no git remote is sent.
fn look_space(gate: &SpaceGate, directory: &Path) -> Result<Option<Place>, String> {
    if gate.directory() != directory || gate.revoked() {
        return Err(NO_TRUST.to_owned());
    }
    let here = std::fs::symlink_metadata(directory).map_err(|_| MOVED.to_owned())?;
    if in_repository(directory) {
        return Err(IN_REPOSITORY.to_owned());
    }
    Ok(identity(&here).map(|directory| Place {
        directory,
        repository: None,
    }))
}

/// Whether `directory`, or one above it, is a repository git or `claude` would find.
fn in_repository(directory: &Path) -> bool {
    (directory.ancestors())
        .any(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok() || bare(dir))
}

/// What is sent of `directory`'s repository as a session there connects, and where it is: under
/// `root`, which must be trusted still.
///
/// # Errors
///
/// Why a session there does not connect, naming no address.
pub fn look(
    root: &TrustedRoot,
    directory: &Path,
    state: Option<&Path>,
) -> Result<(Sent, Option<Place>), String> {
    if root.path() != directory || root.check().is_err() {
        return Err(NO_TRUST.to_owned());
    }
    let here = std::fs::symlink_metadata(directory).map_err(|_| MOVED.to_owned())?;
    let place = |repository| {
        Some(Place {
            directory: identity(&here)?,
            repository,
        })
    };
    if std::fs::symlink_metadata(directory.join(".git")).is_err() {
        // git here never looks above the root, and `claude` may.
        return match in_repository(directory) {
            true => Err(IN_REPOSITORY.to_owned()),
            false => Ok((Sent::default(), place(None))),
        };
    }
    let hooks = empty_hooks(state.ok_or(NO_STATE)?, "remote-control")?;
    let git = Git::new(directory, &hooks).map_err(|_| NO_ANSWER.to_owned())?;
    let common = common_directory(&git, directory)?;
    configured(&git)?;
    let sent = Sent {
        branch: (git.line(&["symbolic-ref", "-q", "--short", "HEAD"]))
            .map(|branch| printable(&branch, MAX_CELLS)),
        ..listed(&git)?
    };
    let repository = std::fs::metadata(&common).ok().as_ref().and_then(identity);
    Ok((sent, repository.and_then(|of| place(Some(of)))))
}

/// Whether `dir` looks like a repository of its own with no `.git`, which git would find.
fn bare(dir: &Path) -> bool {
    ["HEAD", "objects", "refs"]
        .iter()
        .all(|name| dir.join(name).exists())
}

/// The repository's common directory, which holds no remotes of old.
fn common_directory(git: &Git, directory: &Path) -> Result<PathBuf, String> {
    let common = (git.line(&["rev-parse", "--git-common-dir"])).ok_or(NO_ANSWER)?;
    let common = directory.join(common);
    let old = OLD.iter().any(|old| {
        std::fs::read_dir(common.join(old)).is_ok_and(|mut entries| entries.next().is_some())
    });
    match old {
        true => Err(NO_ANSWER.to_owned()),
        false => Ok(common),
    }
}

/// Every address of the configuration, as it is written.
fn configured(git: &Git) -> Result<(), String> {
    let args = ["config", "-z", "--get-regexp", KEYS].map(OsStr::new);
    let out = (git.output(&args, &[], MAX_CONFIG)).map_err(|_| NO_ANSWER.to_owned())?;
    match out.status.code() {
        // None is set.
        Some(1) if out.stdout.is_empty() => return Ok(()),
        Some(0) if !out.cut => {}
        _ => return Err(NO_ANSWER.to_owned()),
    }
    for entry in out.stdout.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let at = entry.iter().position(|b| *b == b'\n').ok_or(NO_ANSWER)?;
        address::judge(&entry[at + 1..]).map_err(|why| why.to_string())?;
    }
    Ok(())
}

/// Every remote's address, as git would use it, rewritten as the configuration says.
fn listed(git: &Git) -> Result<Sent, String> {
    let args = ["remote", "-v"].map(OsStr::new);
    let out = (git.run(&args, &[], MAX_CONFIG)).map_err(|_| NO_ANSWER.to_owned())?;
    let mut addresses: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for line in out.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let address = address_of(line).ok_or(NO_ANSWER)?;
        let address = address::judge(address).map_err(|why| why.to_string())?;
        if seen.insert(address) {
            addresses.push(printable(address, MAX_CELLS));
        }
        if addresses.len() > MAX_SHOWN || address.chars().count() > MAX_CELLS {
            return Err(TOO_MANY.to_owned());
        }
    }
    Ok(Sent {
        addresses,
        branch: None,
    })
}

/// The address of a line of `git remote -v`: `NAME\tADDRESS (fetch)`, with a partial clone's
/// filter after it.
fn address_of(line: &[u8]) -> Option<&[u8]> {
    let at = line.iter().position(|b| *b == b'\t')?;
    let rest = &line[at + 1..];
    let end = rest.iter().position(|b| *b == b' ')?;
    let (address, kind) = rest.split_at(end);
    (kind.starts_with(b" (fetch)") || kind.starts_with(b" (push)")).then_some(address)
}

#[cfg(all(test, unix))]
mod tests;
