//! The diagnostics reports a session keeps (ADR 0138, Bounds, step 6): each document's last, by
//! the path its URI reads as, as the server sent it, so that a document opened later gets it
//! again. At most [`DIAGNOSED_DOCUMENTS_MAX`] documents and [`DIAGNOSTICS_BYTES_MAX`] in all,
//! each report's list as allocated, its strings and its path, held twice, counted; past either,
//! the oldest reports of documents that are not open go first, then the oldest of any.
//!
//! Must not depend on: the session's loop, the pipes or the outbox; this keeps values, and the
//! session thread judges each report before it comes here.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::limits::{DIAGNOSED_DOCUMENTS_MAX, DIAGNOSTICS_BYTES_MAX};
use crate::protocol::Published;

#[cfg(test)]
mod tests;

/// The reports kept, by path, with the order they came in.
#[derive(Debug, Default)]
pub(super) struct Store {
    reports: HashMap<PathBuf, (u64, Arc<Published>)>,
    /// Each kept report's path, by when it came.
    order: BTreeMap<u64, PathBuf>,
    bytes: usize,
    next: u64,
}

/// What a report at `path` costs to keep: the report, and its path in both maps.
fn cost(path: &Path, report: &Published) -> usize {
    report.bytes() + 2 * path.as_os_str().len()
}

impl Store {
    /// Keeps `report`, for the file at `path`, in place of its last; an empty one forgets it.
    /// Past a bound, reports of documents `open` does not say are open go first, oldest first,
    /// in one pass over the order, then the oldest of any; the new one stays.
    pub(super) fn put(&mut self, path: PathBuf, report: Published, open: impl Fn(&Path) -> bool) {
        self.forget(&path);
        if report.diagnostics.is_empty() {
            return;
        }
        let seq = self.next;
        self.next += 1;
        self.bytes += cost(&path, &report);
        self.order.insert(seq, path.clone());
        self.reports.insert(path, (seq, Arc::new(report)));
        let over = |len: usize, bytes: usize| {
            len > DIAGNOSED_DOCUMENTS_MAX || bytes > DIAGNOSTICS_BYTES_MAX
        };
        let (mut len, mut bytes) = (self.reports.len(), self.bytes);
        let mut victims = Vec::new();
        // First those not open, then any: the second pass takes only open ones, as the first
        // took every one that is not open before it stopped.
        for open_ones in [false, true] {
            for (kept, path) in &self.order {
                if !over(len, bytes) {
                    break;
                }
                if *kept == seq || open(path) != open_ones {
                    continue;
                }
                if let Some((_, report)) = self.reports.get(path) {
                    bytes -= cost(path, report);
                    len -= 1;
                    victims.push(path.clone());
                }
            }
        }
        for path in victims {
            self.forget(&path);
        }
    }

    /// The report kept for the file at `path`, if any, shared.
    pub(super) fn get(&self, path: &Path) -> Option<Arc<Published>> {
        self.reports.get(path).map(|(_, report)| Arc::clone(report))
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
            self.bytes -= cost(path, &report);
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
