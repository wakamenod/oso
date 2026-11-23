//! A native session's gate in an issue space's own directory (ADR 0176): the space's
//! directory is its own root there, judged by the space's record and the directory oso made,
//! never by git's trust. No [`TrustedRoot`](crate::git::TrustedRoot) is made for it, so
//! nothing lands in the sessions' tool trusts, and no baseline is taken.
//!
//! Must not depend on: `app`. [`judge`] is pure; [`settle`] runs on the start thread and reads
//! nothing of the disk itself: the space's checks (`check_before_start`) look at the
//! directory, its identity among them, just before.
//!
//! - Only in an open space's recorded directory, where oso made it under `[spaces] root`
//!   (`Stored::orchestrated_from`).
//! - Only for its current orchestrator, an earlier one, or a session of the person's there,
//!   which a fork of one of them is (the author's Q-O3 (a)); a fork is judged by the session
//!   it copies. A new orchestrator oso starts there is the space's too (part O-3).
//! - The UI takes the gate back after each event once the space closed, its directory
//!   changed, or the session is no longer the space's; the start thread looks at it again
//!   just before the spawn.
//! - A gate judged while starts wait for the configuration's first look (ADR 0173) waits
//!   too, as `[spaces] root` is not known yet: the held order is judged again.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::trust::ByRecord;
use super::{Order, Runs};
use crate::sessions::identity::SessionId;
use crate::sessions::spaces::Stored;

/// Why a session in a space's directory does not run natively there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAdmitted {
    /// The space is closed.
    Closed,
    /// `[spaces] root` is not known, or the directory is not where oso made it under the root
    /// with an identity kept.
    NotMade,
    /// The session is neither the space's current orchestrator, nor an earlier one, nor a
    /// session of the person's there.
    NotTheSpaces,
}

impl NotAdmitted {
    /// What the echo area says, after "Session NAME ".
    pub fn reason(self) -> &'static str {
        match self {
            Self::Closed => "is in a closed space's directory",
            Self::NotMade => {
                "is in a space's directory that is not where oso made it under [spaces] root"
            }
            Self::NotTheSpaces => {
                "is in a space's directory, but is not its orchestrator, an earlier one or a fork \
                 of one"
            }
        }
    }
}

/// The session a gate is judged for.
#[derive(Debug, Clone, Copy)]
pub enum Standing<'a> {
    /// A resume's own conversation, or the one a fork copies.
    Of {
        id: &'a SessionId,
        /// It is a session the person started in the space's directory, not an agent's: a
        /// fork of an orchestrator there is one.
        persons_there: bool,
    },
    /// The space's new orchestrator, which oso starts there now (`space-orchestrator-start`,
    /// `space-new`'s start or a successor's; part O-3), with no conversation yet.
    NewOrchestrator,
}

/// A native session's leave to run in a space's directory, until it is taken back.
#[derive(Debug, Clone)]
pub struct SpaceGate {
    directory: PathBuf,
    /// The directory's identity when oso made it, as the space's checks compare it.
    made: Option<(u64, u64)>,
    revoked: Arc<AtomicBool>,
}

impl SpaceGate {
    /// The directory it was judged in.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Takes it back: the start thread's look before the spawn refuses it.
    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
    }

    /// Whether it was taken back.
    pub fn revoked(&self) -> bool {
        self.revoked.load(Ordering::SeqCst)
    }
}

/// What a native order's gate in a space's directory is on the start thread.
#[derive(Debug, Default)]
pub enum ByGate {
    /// None: the order is judged by its project's trust alone.
    #[default]
    No,
    /// The space's gate.
    Gate(SpaceGate),
    /// Judged while starts wait for the configuration: the UI holds such an order until they
    /// do not, and judges it again, so this refuses.
    Waits,
}

/// The gate of a native session `standing` in `directory`, the recorded directory of space
/// `stored`, with `[spaces] root` at `root`.
///
/// # Errors
///
/// Why it does not run natively there.
pub fn judge(
    stored: &Stored,
    root: Option<&Path>,
    directory: &Path,
    standing: Standing<'_>,
) -> Result<SpaceGate, NotAdmitted> {
    if stored.archived {
        return Err(NotAdmitted::Closed);
    }
    if !root.is_some_and(|root| stored.orchestrated_from(root, directory)) {
        return Err(NotAdmitted::NotMade);
    }
    let theirs = match standing {
        Standing::Of { id, persons_there } => {
            stored.orchestrator.as_ref() == Some(id) || stored.earlier.contains(id) || persons_there
        }
        Standing::NewOrchestrator => true,
    };
    if !theirs {
        return Err(NotAdmitted::NotTheSpaces);
    }
    Ok(SpaceGate {
        directory: directory.to_owned(),
        made: stored.directory_identity,
        revoked: Arc::default(),
    })
}

/// Whether `order` waits on the UI thread to have its gate judged.
pub fn waits(order: &Order) -> bool {
    matches!(&order.runs, Runs::Native(native) if matches!(native.gate, ByGate::Waits))
}

/// The gate a native `order` carries, taken from it on the start thread, once the space's
/// checks passed: still not taken back, for the directory the order starts in, which the
/// checks looked at with the identity the gate was judged with. `None` for an order with no
/// gate.
///
/// # Errors
///
/// Why it does not start, for a person.
pub(super) fn settle(order: &mut Order) -> Result<Option<SpaceGate>, String> {
    let Runs::Native(native) = &mut order.runs else {
        return Ok(None);
    };
    let gate = match std::mem::take(&mut native.gate) {
        ByGate::No => return Ok(None),
        ByGate::Gate(gate) => gate,
        ByGate::Waits => {
            return Err(
                "The session did not start: its space's directory is not judged yet".into(),
            );
        }
    };
    let checked = (order.space.as_ref())
        .is_some_and(|space| space.in_space_directory && space.made == gate.made);
    // Never both a project's trust and a space's: the gate's line reads other settings.
    let trusted = order.trusted.is_some() || !matches!(native.trust, ByRecord::No);
    if !checked || order.directory != gate.directory || trusted {
        return Err(
            "The session did not start: it is not in the space's directory it was judged in".into(),
        );
    }
    if gate.revoked() {
        return Err(TAKEN_BACK.into());
    }
    Ok(Some(gate))
}

/// What a start whose gate was taken back says.
pub(super) const TAKEN_BACK: &str = "The session did not start: its space closed, its directory \
                                     changed, or it is no longer the space's";

#[cfg(test)]
mod tests;
