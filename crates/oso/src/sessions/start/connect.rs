//! A native session's start that Remote Control may connect (ADR 0181): what is sent of its
//! repository as it connects, read by git, and whether it may be; and that it is where it was,
//! for a session connected again by `oso-restart`.
//!
//! Must not depend on: `app`. [`look`] and [`settle`] run git and read the disk, so they run on
//! threads of their own: a start thread, and the one before the person's question.
//!
//! - A start is [`Remote::Blocked`] unless the app gives it a [`Connect`], which is made for one
//!   start and cannot be copied: at the person's `y`, or from the parent's restart state.
//! - Judged just before the line is built, under the trust the start was judged by; never under
//!   a space's gate. Refused, it starts blocked, and says why.
//! - Every address the repository's configuration names is read, what a rewrite makes of it too,
//!   and each must pass (`address`). A git that does not answer refuses, and so does a
//!   directory below another's repository, which git here is kept from and `claude` may find.
//! - What a refusal says names no address and no remote. Between this look and the spawn, and as
//!   long as the session runs, what its own tools make of the configuration is not seen.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::{Order, Runs};
use crate::git::{Git, TrustedRoot, empty_hooks, identity, printable};
use crate::sessions::native::wire::remote::RemoteId;

pub mod address;

/// The most bytes of the configuration's remotes read.
const MAX_CONFIG: usize = 256 * 1024;

/// The most addresses shown.
const MAX_SHOWN: usize = 8;

/// The most characters of an address or a branch shown.
const MAX_CELLS: usize = 200;

/// The configuration's keys an address, or a remote's name that may be one, is in.
const KEYS: &str =
    r"^(remote\..*\.(url|pushurl)|remote\.pushdefault|branch\..*\.(remote|pushremote))$";

/// Where git before its configuration kept remotes.
const OLD: [&str; 2] = ["remotes", "branches"];

const NO_ANSWER: &str = "git did not say what the repository's remotes are";
const MOVED: &str = "its directory or its repository is not the one it was connected in";
const NO_TRUST: &str = "its project is not trusted now";
const NO_STATE: &str = "oso has no state directory, which its git needs";

/// Whether a native start's Remote Control is blocked.
#[derive(Debug, Default)]
pub enum Remote {
    /// The line blocks it: every start but the two a [`Connect`] is made for.
    #[default]
    Blocked,
    /// The line does not.
    Connect(Connect),
}

/// Leave for one start not to block Remote Control.
#[derive(Debug)]
pub struct Connect {
    /// The session was connected before `oso-restart`.
    back: Option<Back>,
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
    /// More than are shown.
    pub more: usize,
    pub branch: Option<String>,
}

/// How a native start's Remote Control was settled, for the app.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum Settled {
    #[default]
    Blocked,
    /// It was to connect, and started blocked: why.
    Refused(String),
    /// Not blocked: where it runs, where the system says so.
    Connects(Option<Place>),
}

impl Connect {
    /// Leave at the person's `y`.
    pub fn asked() -> Self {
        Self { back: None }
    }

    /// Leave to come back to `back`, which the parent's restart state carried.
    pub fn again(back: Back) -> Self {
        Self { back: Some(back) }
    }

    /// The session's id there, if it comes back.
    pub fn back(&self) -> Option<&RemoteId> {
        self.back.as_ref().map(|back| &back.id)
    }
}

impl Remote {
    /// Whether the line blocks it.
    pub fn blocked(&self) -> bool {
        matches!(self, Self::Blocked)
    }
}

/// Settles native `order`'s Remote Control, its trust judged already: blocked unless every check
/// passes.
pub(super) fn settle(order: &mut Order) -> Settled {
    let Runs::Native(native) = &mut order.runs else {
        return Settled::Blocked;
    };
    let Remote::Connect(connect) = std::mem::take(&mut native.remote) else {
        return Settled::Blocked;
    };
    let judged = match &order.trusted {
        Some(root) => judge(root, &order.directory, order.state.as_deref(), &connect),
        None => Err(NO_TRUST.to_owned()),
    };
    match judged {
        Ok(place) => {
            native.remote = Remote::Connect(connect);
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
    let (_, place) = look(root, directory, state)?;
    match &connect.back {
        Some(back) if place != Some(back.place) => Err(MOVED.to_owned()),
        _ => Ok(place),
    }
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
        let above = (directory.ancestors().skip(1))
            .any(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok());
        return match above {
            true => Err("its directory is below another repository".to_owned()),
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
    for line in out.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let address = address_of(line).ok_or(NO_ANSWER)?;
        let address = address::judge(address).map_err(|why| why.to_string())?;
        let shown = printable(address, MAX_CELLS);
        if !addresses.contains(&shown) {
            addresses.push(shown);
        }
    }
    let more = addresses.len().saturating_sub(MAX_SHOWN);
    addresses.truncate(MAX_SHOWN);
    Ok(Sent {
        addresses,
        more,
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
