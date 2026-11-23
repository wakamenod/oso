//! A native session's trust (ADR 0165, Q5, parts 2b and 4a): judged on the UI thread from an
//! open buffer of its project, and, for a resume or a fork with none open, from the record of
//! trusted projects by its root's path; then on the start thread by that root's path and
//! identity, as an issue space's member is (`TrustedRoot::of_member`, ADR 0136, step 4).
//!
//! Must not depend on: `app`. [`judge`] is pure; [`settle`] reads the root's identity from the
//! disk, so only the start thread calls it.
//!
//! - Only at its project's root.
//! - An open buffer of the project decides: open and not trusted, or its trust taken back,
//!   refuses, whatever the record says.
//! - With none open, a resume's or a fork's record decides, by path here and by path and
//!   identity on the start thread, so a directory made again at the path is refused. A new
//!   start's never does: it needs an open, trusted project, as before.
//! - A record not read yet waits while starts may wait for it (ADR 0173); one refused, or not
//!   read in time, trusts nothing.
//! - A linked worktree's trust through its main repository (ADR 0114) comes only from an open
//!   buffer: the record holds the main repository, not the worktree.

use std::path::{Path, PathBuf};

use oso_core::project::TrustList;

use super::{Order, Runs};
use crate::git::TrustedRoot;

/// Why a native session does not run natively where it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotTrusted {
    /// No buffer of a project at its root is open, so its trust is not known now: a new
    /// start's only.
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
pub enum Record<'a> {
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
    /// The record is not read yet: the order waits for it.
    Waits,
}

/// A root the record of trusted projects holds by path, as the UI read it.
#[derive(Debug, Clone)]
pub struct Listed {
    list: TrustList,
    root: PathBuf,
}

/// What a native order's trust is judged by on the start thread, besides `Order::trusted`.
#[derive(Debug, Default)]
pub enum ByRecord {
    /// `Order::trusted` alone.
    #[default]
    No,
    /// The record, for the root it holds by path.
    Judge(Listed),
    /// The record was not read: the UI holds such an order until it is, so this refuses.
    Unread,
}

/// The trust a native session in `root` (`at_root` when it runs there) runs under, as an open
/// buffer of its project has it now (`open`: `None` when none is open, `Some(None)` when it is
/// not trusted) or, for a resume or a fork, as `record` has it. A record not read waits only
/// while `may_wait`.
///
/// # Errors
///
/// Why it does not run natively.
pub fn judge(
    at_root: bool,
    root: &Path,
    open: Option<Option<TrustedRoot>>,
    record: Option<Record<'_>>,
    may_wait: bool,
) -> Result<Judged, NotTrusted> {
    if !at_root {
        return Err(NotTrusted::NotAtRoot);
    }
    match (open, record) {
        (Some(None), _) => Err(NotTrusted::Untrusted),
        (Some(Some(trusted)), _) if trusted.revoked() => Err(NotTrusted::Untrusted),
        (Some(Some(trusted)), _) => Ok(Judged::Open(trusted)),
        (None, None) => Err(NotTrusted::NotOpen),
        (None, Some(Record::Read(list))) if list.paths().any(|path| path == root) => {
            Ok(Judged::Listed(Listed {
                list: list.clone(),
                root: root.to_owned(),
            }))
        }
        (None, Some(Record::Read(_))) => Err(NotTrusted::Untrusted),
        (None, Some(Record::Unread)) if may_wait => Ok(Judged::Waits),
        (None, Some(Record::Unread | Record::Refused)) => Err(NotTrusted::NoRecord),
    }
}

impl Judged {
    /// The order's `trusted` and what its start thread judges by the record.
    pub fn into_order(self) -> (Option<TrustedRoot>, ByRecord) {
        match self {
            Judged::Open(trusted) => (Some(trusted), ByRecord::No),
            Judged::Listed(listed) => (None, ByRecord::Judge(listed)),
            Judged::Waits => (None, ByRecord::Unread),
        }
    }
}

/// Whether `order` waits for the record of trusted projects on the UI thread.
pub fn waits(order: &Order) -> bool {
    matches!(&order.runs, Runs::Native(native) if matches!(native.trust, ByRecord::Unread))
}

impl Listed {
    /// The root, if the record trusts the directory there now by its path and identity: not
    /// following a link, and a directory. It looks at the disk.
    fn look(&self) -> Option<TrustedRoot> {
        let metadata = std::fs::symlink_metadata(&self.root).ok()?;
        if !metadata.is_dir() {
            return None;
        }
        let identity = crate::git::identity(&metadata);
        TrustedRoot::of_member(&self.list, &self.root, identity)
    }
}

/// Judges a native `order` by the record it carries, on the start thread: its `trusted` is
/// the root the record trusts there now, else it is refused.
///
/// # Errors
///
/// Why it does not start, for a person.
pub(super) fn settle(order: &mut Order) -> Result<(), String> {
    let Runs::Native(native) = &mut order.runs else {
        return Ok(());
    };
    match std::mem::take(&mut native.trust) {
        ByRecord::No => Ok(()),
        ByRecord::Judge(listed) => {
            order.trusted = Some(listed.look().ok_or_else(|| {
                "The session did not start: the record of trusted projects does not hold the \
                 directory at its project's root now; trust-project trusts the one there now, \
                 if it is yours"
                    .to_owned()
            })?);
            Ok(())
        }
        ByRecord::Unread => {
            Err("The session did not start: the record of trusted projects is not read".into())
        }
    }
}

#[cfg(test)]
mod tests;
