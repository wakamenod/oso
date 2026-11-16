//! The diagnostics reports a session keeps (ADR 0138, Bounds, step 6): each document's last, by
//! the path its URI reads as, as the server sent it, so that a document opened later gets it again. At most
//! [`DIAGNOSED_DOCUMENTS_MAX`] documents and [`DIAGNOSTICS_BYTES_MAX`] in all; past either, the
//! oldest report of a document that is not open goes first, then the oldest of any.
//!
//! Must not depend on: the session's loop, the pipes or the outbox; this keeps values, and the
//! session thread judges each report before it comes here.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::limits::{DIAGNOSED_DOCUMENTS_MAX, DIAGNOSTICS_BYTES_MAX};
use crate::protocol::Published;

#[cfg(test)]
mod tests;

/// The reports kept, by path, with the order they came in.
#[derive(Debug, Default)]
pub(super) struct Store {
    reports: HashMap<PathBuf, (u64, Published)>,
    /// Each kept report's path, by when it came.
    order: BTreeMap<u64, PathBuf>,
    bytes: usize,
    next: u64,
}

impl Store {
    /// Keeps `report`, for the file at `path`, in place of its last; an empty one forgets it.
    /// Past a bound, reports of documents `open` does not say are open go first, oldest first.
    pub(super) fn put(&mut self, path: PathBuf, report: Published, open: impl Fn(&Path) -> bool) {
        self.forget(&path);
        if report.diagnostics.is_empty() {
            return;
        }
        let seq = self.next;
        self.next += 1;
        self.bytes += report.bytes();
        self.order.insert(seq, path.clone());
        self.reports.insert(path, (seq, report));
        while self.reports.len() > DIAGNOSED_DOCUMENTS_MAX || self.bytes > DIAGNOSTICS_BYTES_MAX {
            let oldest = self
                .order
                .iter()
                .find(|(kept, path)| **kept != seq && !open(path))
                .or_else(|| self.order.iter().find(|(kept, _)| **kept != seq))
                .map(|(_, path)| path.clone());
            let Some(path) = oldest else {
                break;
            };
            self.forget(&path);
        }
    }

    /// The report kept for the file at `path`, if any.
    pub(super) fn get(&self, path: &Path) -> Option<&Published> {
        self.reports.get(path).map(|(_, report)| report)
    }

    /// Forgets every report, as a run ends: the next run reports again.
    pub(super) fn clear(&mut self) {
        *self = Self {
            next: self.next,
            ..Self::default()
        };
    }

    fn forget(&mut self, path: &Path) {
        if let Some((seq, report)) = self.reports.remove(path) {
            self.order.remove(&seq);
            self.bytes -= report.bytes();
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.reports.len()
    }

    #[cfg(test)]
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }
}
