//! A native session's trust (ADR 0165, Q5, parts 2b and 4a): judged on the UI thread from an
//! open buffer of its project, and, for a resume or a fork with none open, from the record of
//! trusted projects by its root's path; then on the start thread by that root's path and
//! identity, as an issue space's member is (`TrustedRoot::of_member`, ADR 0136, step 4), or,
//! for a resume in a linked worktree, by what git's files there say now ([`Claimed`]).
//!
//! Must not depend on: `app`. [`judge`] is pure; [`settle`] and [`Admitted::again`] read the
//! disk, so only the start thread calls them.
//!
//! - Only at its project's root.
//! - An open buffer of the project decides: open and not trusted, or its trust taken back,
//!   refuses, whatever the record says; a resume asks it only once the record is read.
//! - With none open, a resume's or a fork's record decides, by path here and by path and
//!   identity on the start thread, so a directory made again at the path is refused. A new
//!   start's never does: it needs an open, trusted project, as before.
//! - A record not read yet waits while starts may wait for it (ADR 0173); one refused, or not
//!   read in time, trusts nothing.
//! - The record holds the main repository of a linked worktree, not the worktree. A new start,
//!   a fork, an adoption or a move into a space there needs an open buffer of it. A resume whose
//!   root the record does not hold is judged by the worktree's claim (ADR 0114), walked afresh
//!   on the start thread, whole and again just before the spawn ([`Claims`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oso_core::project::{TrustList, same_place};

use super::{NOT_STARTED, Order, Runs};
use crate::git::TrustedRoot;

/// Why a native session does not run natively where it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotTrusted {
    /// No buffer of a project at its root is open, so its trust is not known now: for a start
    /// judged by the record, and the record does not hold its root either, as a linked
    /// worktree's, whose main repository it holds. A resume asks the worktree's claim instead.
    NotOpen,
    /// The project at its root is not trusted now.
    Untrusted,
    /// It runs elsewhere than at its project's root.
    NotAtRoot,
    /// No buffer of its project is open, and the record of trusted projects was refused, is
    /// broken, or was not read in time.
    NoRecord,
}

/// The record of trusted projects as the UI holds it.
#[derive(Debug, Clone, Copy)]
pub enum TrustRecord<'a> {
    Read(&'a TrustList),
    /// Its thread has not reported yet.
    Unread,
    /// It was refused, is broken, or there is none to read: it trusts nothing.
    Refused,
}

/// Where a native session's trust comes from.
#[derive(Debug)]
pub enum Judged {
    /// An open buffer of its project trusts it now: the order's `trusted`.
    Open(TrustedRoot),
    /// The record holds its root's path: the start thread checks its identity too.
    Listed(Listed),
    /// The record does not hold its root's path: the start thread asks whether the root is a
    /// linked worktree of a repository the record trusts.
    Claimed(Claimed),
    /// The record is not read yet: the order waits for it.
    Waits,
}

/// A root the record of trusted projects holds by path, as the UI read it.
#[derive(Debug, Clone)]
pub struct Listed {
    list: TrustList,
    root: PathBuf,
    /// Shared with what the UI keeps of the start ([`Kept`]), which takes it back once the
    /// project is no longer trusted, and with the root the start thread makes.
    revoked: Arc<AtomicBool>,
}

/// Whether a start with no buffer of its project open may be judged by its worktree's claim.
#[derive(Debug, Clone, Copy)]
pub enum Claims<'a> {
    /// Never: a new start, a fork, an adoption or a move into a space.
    Never,
    /// A resume of a session oso has. `home`, as it is spelled, is never a project.
    Asked { home: Option<&'a Path> },
}

/// A root the record does not hold by path, which may be a linked worktree of a repository it
/// trusts: the start thread judges it by what git's files there say, read afresh.
#[derive(Debug, Clone)]
pub struct Claimed {
    /// Shared with what the UI keeps of the start.
    list: Arc<TrustList>,
    root: PathBuf,
    home: Option<PathBuf>,
    /// As [`Listed`]'s.
    revoked: Arc<AtomicBool>,
}

/// What the UI keeps of a native resume or fork until its start thread reports, to take its
/// trust back once its project is no longer trusted: the start thread's look again before the
/// spawn then refuses it.
#[derive(Debug, Clone)]
pub struct Kept {
    root: PathBuf,
    open: Option<TrustedRoot>,
    /// The record as it was read for a [`Claimed`] start.
    claimed: Option<Arc<TrustList>>,
    revoked: Arc<AtomicBool>,
}

impl Kept {
    /// The root it was judged at.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether it still stands, as an open buffer of its project (`open`) or else `record` has
    /// it now. One judged by its worktree's claim is taken back by an open buffer that is not
    /// trusted, and once any root of the record it was judged by is recorded no longer: which
    /// one the claim names is known to its start thread alone.
    pub fn still_trusted(
        &self,
        open: Option<Option<TrustedRoot>>,
        record: TrustRecord<'_>,
    ) -> bool {
        match &self.claimed {
            Some(_) if matches!(&open, Some(None)) => false,
            Some(list) => stands(None, record, |now| holds_all(now, list)),
            None => still_trusted(&self.root, open, record),
        }
    }

    /// Takes its trust back.
    pub fn revoke(&self) {
        if let Some(open) = &self.open {
            open.revoke();
        }
        self.revoked.store(true, Ordering::SeqCst);
    }
}

/// What a native order's trust is judged by on the start thread, besides `Order::trusted`.
#[derive(Debug, Default)]
pub enum ByRecord {
    /// `Order::trusted` alone.
    #[default]
    No,
    /// The record, for the root it holds by path.
    Judge(Listed),
    /// The worktree's claim, for a root it does not.
    Claim(Claimed),
    /// The record was not read: the UI holds such an order until it is, so this refuses.
    Unread,
}

/// The trust a native session in `root` (`at_root` when it runs there) runs under, as an open
/// buffer of its project has it now (`open`: `None` when none is open, `Some(None)` when it is
/// not trusted) or, for a resume or a fork, as `record` has it, and where `claims` asks, as its
/// worktree's claim does. A record not read waits only while `may_wait`.
///
/// A resume asked of its claim waits for a record not read before an untrusted buffer of its
/// project refuses, as no project is trusted until it is. A buffer of a worktree is asked again
/// only once it is read, so it may still refuse then.
///
/// # Errors
///
/// Why it does not run natively.
pub fn judge(
    at_root: bool,
    root: &Path,
    open: Option<Option<TrustedRoot>>,
    record: Option<TrustRecord<'_>>,
    may_wait: bool,
    claims: Claims<'_>,
) -> Result<Judged, NotTrusted> {
    if !at_root {
        return Err(NotTrusted::NotAtRoot);
    }
    match (open, record) {
        (Some(None), Some(TrustRecord::Unread))
            if may_wait && matches!(claims, Claims::Asked { .. }) =>
        {
            Ok(Judged::Waits)
        }
        (Some(None), _) => Err(NotTrusted::Untrusted),
        (Some(Some(trusted)), _) if trusted.revoked() => Err(NotTrusted::Untrusted),
        (Some(Some(trusted)), _) => Ok(Judged::Open(trusted)),
        (None, None) => Err(NotTrusted::NotOpen),
        (None, Some(TrustRecord::Read(list))) if list.paths().any(|path| path == root) => {
            Ok(Judged::Listed(Listed {
                list: list.clone(),
                root: root.to_owned(),
                revoked: Arc::default(),
            }))
        }
        // Not trusted, or a linked worktree, which an open buffer of it, or for a resume its
        // claim, may trust (ADR 0114).
        (None, Some(TrustRecord::Read(list))) => match claims {
            Claims::Asked { home } => Ok(Judged::Claimed(Claimed {
                list: Arc::new(list.clone()),
                root: root.to_owned(),
                home: home.map(Path::to_owned),
                revoked: Arc::default(),
            })),
            Claims::Never => Err(NotTrusted::NotOpen),
        },
        (None, Some(TrustRecord::Unread)) if may_wait => Ok(Judged::Waits),
        (None, Some(TrustRecord::Unread | TrustRecord::Refused)) => Err(NotTrusted::NoRecord),
    }
}

/// Whether a native start judged at `root` is still trusted, as an open buffer of its project
/// (`open`) or else `record` has it now: by path alone, as the UI has no identity to look at.
/// A record not read yet keeps it, as its order waits for it and is judged again.
pub fn still_trusted(
    root: &Path,
    open: Option<Option<TrustedRoot>>,
    record: TrustRecord<'_>,
) -> bool {
    let open = open.map(|open| open.is_some_and(|trusted| !trusted.revoked()));
    stands(open, record, |list| list.paths().any(|path| path == root))
}

/// Whether a trust given stands, as an open buffer of its project (`open`) has it, else as
/// `listed` says of `record`. A record not read yet keeps it; one refused trusts nothing.
pub fn stands(
    open: Option<bool>,
    record: TrustRecord<'_>,
    listed: impl FnOnce(&TrustList) -> bool,
) -> bool {
    match (open, record) {
        (Some(open), _) => open,
        (None, TrustRecord::Read(list)) => listed(list),
        (None, TrustRecord::Unread) => true,
        (None, TrustRecord::Refused) => false,
    }
}

/// Whether `now` still holds every root of `list`, by path and identity.
fn holds_all(now: &TrustList, list: &TrustList) -> bool {
    if now == list {
        return true;
    }
    // A line of the record is a root's path and identity, and nothing else.
    let now = now.format();
    let now: HashSet<&str> = now.lines().collect();
    list.format().lines().all(|root| now.contains(root))
}

impl Judged {
    /// What the UI keeps of the start, to take its trust back: `None` while it waits.
    pub fn kept(&self) -> Option<Kept> {
        match self {
            Judged::Open(trusted) => Some(Kept {
                root: trusted.path().to_owned(),
                open: Some(trusted.clone()),
                claimed: None,
                revoked: Arc::default(),
            }),
            Judged::Listed(listed) => Some(Kept {
                root: listed.root.clone(),
                open: None,
                claimed: None,
                revoked: Arc::clone(&listed.revoked),
            }),
            Judged::Claimed(claimed) => Some(Kept {
                root: claimed.root.clone(),
                open: None,
                claimed: Some(Arc::clone(&claimed.list)),
                revoked: Arc::clone(&claimed.revoked),
            }),
            Judged::Waits => None,
        }
    }

    /// The order's `trusted` and what its start thread judges by the record.
    pub fn into_order(self) -> (Option<TrustedRoot>, ByRecord) {
        match self {
            Judged::Open(trusted) => (Some(trusted), ByRecord::No),
            Judged::Listed(listed) => (None, ByRecord::Judge(listed)),
            Judged::Claimed(claimed) => (None, ByRecord::Claim(claimed)),
            Judged::Waits => (None, ByRecord::Unread),
        }
    }
}

/// The root `judged` trusts now, looked at on a thread of its own: what a session at it would
/// resume under (ADR 0181).
///
/// # Errors
///
/// Why none, for a person.
pub fn look(judged: Judged) -> Result<TrustedRoot, String> {
    match judged {
        Judged::Open(root) => root.check().map(|()| root),
        Judged::Listed(listed) => listed
            .look()
            .ok_or_else(|| "it is not trusted now".to_owned()),
        Judged::Claimed(claimed) => claimed.look().map_err(|why| why.reason().to_owned()),
        Judged::Waits => Err("the record of trusted projects is not read yet".to_owned()),
    }
}

/// Whether `order` waits for the record of trusted projects on the UI thread.
pub fn waits(order: &Order) -> bool {
    matches!(&order.runs, Runs::Native(native) if matches!(native.trust, ByRecord::Unread))
}

impl Listed {
    /// The root, if the record trusts the directory there now by its path and identity: not
    /// following a link, and a directory, and not taken back since. Its revocation is the one
    /// the UI holds. It looks at the disk.
    fn look(&self) -> Option<TrustedRoot> {
        if self.revoked.load(Ordering::SeqCst) {
            return None;
        }
        let metadata = std::fs::symlink_metadata(&self.root).ok()?;
        if !metadata.is_dir() {
            return None;
        }
        let identity = crate::git::identity(&metadata);
        TrustedRoot::of_member(&self.list, &self.root, identity)
            .map(|root| root.revoked_by(Arc::clone(&self.revoked)))
    }
}

/// Why a root is not trusted by its worktree's claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoClaim {
    /// Taken back since it was judged.
    Revoked,
    /// Its claim names a repository the record does not hold.
    Unlisted,
    /// Anything else. What its `.git` names is never shown.
    NotOne,
}

/// What a claim refused as [`NoClaim::Unlisted`] and as [`NoClaim::NotOne`] says.
pub const UNLISTED: &str =
    "its repository is not trusted now: trust-project it, then session-resume";
pub const NOT_ONE: &str = "its directory is neither a trusted repository nor a worktree of one now";

/// What a root the record does not hold as the directory there says.
pub const NOT_RECORDED: &str = "The session did not start: the record of trusted projects does \
                                not hold the directory at its project's root now, or it is no \
                                longer trusted; trust-project trusts the one there now, if it is \
                                yours";

impl NoClaim {
    fn reason(self) -> &'static str {
        match self {
            Self::Revoked => "its project is no longer trusted",
            Self::Unlisted => UNLISTED,
            Self::NotOne => NOT_ONE,
        }
    }

    /// What the start says.
    fn said(self) -> String {
        format!("{NOT_STARTED}{}", self.reason())
    }
}

impl Claimed {
    /// The root, if it is, as the disk has it now, a linked worktree of a repository the record
    /// trusts (ADR 0114): no link on the way to it, its own root, never the home directory, its
    /// claim checked both ways, and the main repository recorded by path and identity. Its
    /// revocation is the one the UI holds. It runs no git, and looks at the disk.
    fn look(&self) -> Result<TrustedRoot, NoClaim> {
        if self.revoked.load(Ordering::SeqCst) {
            return Err(NoClaim::Revoked);
        }
        let home = (self.home.as_ref()).and_then(|home| std::fs::canonicalize(home).ok());
        let roots: Vec<PathBuf> = self.list.paths().map(PathBuf::from).collect();
        let found = crate::project::detect_root(&self.root, home.as_deref(), &roots)
            // It resolves the root: a link there or on the way to it leads elsewhere.
            .filter(|found| found.root.path == self.root)
            .ok_or(NoClaim::NotOne)?;
        let unlisted = |main: &PathBuf| !roots.iter().any(|root| same_place(root, main));
        match TrustedRoot::of_claim(&self.list, &found) {
            Some(root) => Ok(root.revoked_by(Arc::clone(&self.revoked))),
            None if found.worktree_of.as_ref().is_some_and(unlisted) => Err(NoClaim::Unlisted),
            None => Err(NoClaim::NotOne),
        }
    }
}

/// A root admitted by its worktree's claim, until just before the spawn.
#[derive(Debug)]
pub(super) struct Admitted {
    claimed: Claimed,
    identity: Option<(u64, u64)>,
}

impl Admitted {
    /// The whole look again: the root must still be a worktree of a trusted repository, and the
    /// directory it was.
    ///
    /// # Errors
    ///
    /// Why it does not start, for a person.
    pub(super) fn again(&self) -> Result<(), String> {
        match self.claimed.look() {
            Ok(root) if root.identity() == self.identity => Ok(()),
            Ok(_) => Err(NoClaim::NotOne.said()),
            Err(why) => Err(why.said()),
        }
    }
}

/// Judges a native `order` by the record it carries, on the start thread: its `trusted` is
/// the root the record trusts there now, else it is refused. One judged by its worktree's
/// claim is looked at again just before the spawn, by what is returned.
///
/// # Errors
///
/// Why it does not start, for a person.
pub(super) fn settle(order: &mut Order) -> Result<Option<Admitted>, String> {
    let Runs::Native(native) = &mut order.runs else {
        return Ok(None);
    };
    match std::mem::take(&mut native.trust) {
        ByRecord::No => Ok(None),
        ByRecord::Judge(listed) => {
            order.trusted = Some(listed.look().ok_or_else(|| NOT_RECORDED.to_owned())?);
            Ok(None)
        }
        ByRecord::Claim(claimed) => {
            let root = claimed.root.as_path();
            // Judged where it runs, as the project it runs in.
            if order.directory != root || order.project.as_deref() != Some(root) {
                return Err(NoClaim::NotOne.said());
            }
            let trusted = claimed.look().map_err(NoClaim::said)?;
            let identity = trusted.identity();
            order.trusted = Some(trusted);
            Ok(Some(Admitted { claimed, identity }))
        }
        ByRecord::Unread => {
            Err("The session did not start: the record of trusted projects is not read".into())
        }
    }
}

#[cfg(test)]
mod tests;
