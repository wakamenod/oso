//! The texts a document was given last, kept to place a late diagnostics report against the
//! text it names and map it forward to the latest (ADR 0138, Document sync and Diagnostics,
//! step 6). At most [`KEPT_VERSIONS_MAX`] of them, sharing their chunks with the buffer's, and
//! only those a report can still use.
//!
//! Must not depend on: the session's loop, the pipes or the outbox; this keeps text and builds
//! values, and the session thread hands them on.
//!
//! - A text whose splices from the one before are not known, a whole text, lets those before it
//!   go: nothing could be mapped through it.
//! - A document kept from the server, or not sent yet, keeps only its latest text: no report
//!   can name one of its earlier ones.
//! - A report naming a version lets the texts before it go.

use std::path::Path;

use oso_core::diagnostic::{Diagnostics, New};
use oso_core::rope::Rope;
use oso_core::syntax::{Splice, compose};

use super::{Documents, Kept};
use crate::limits::KEPT_VERSIONS_MAX;
use crate::position::{self, Encoding};
use crate::protocol::Published;

#[cfg(test)]
mod tests;

/// A text the caller gave, kept to map a late report forward.
#[derive(Debug)]
pub(super) struct Given {
    /// The caller's revision of it.
    pub(super) revision: u64,
    pub(super) text: Rope,
    /// The splices from the text given before it, if they placed.
    pub(super) splices: Option<Vec<Splice>>,
    /// The version it went to the server as, once it went.
    pub(super) version: Option<i32>,
}

/// What a report comes to for the documents.
#[derive(Debug)]
pub(in crate::server) enum Diagnose {
    /// The set for document `id`, placed against the text the report names and mapped to the
    /// text given last, of revision `revision`.
    For {
        id: u64,
        revision: u64,
        diagnostics: Diagnostics,
    },
    /// No document open with the server is at its path.
    NotOpen,
    /// It names a version no longer kept, or one a whole text came after, which it cannot be
    /// mapped through: a newer one follows.
    Stale,
}

impl Kept {
    /// Keeps `given` as the latest text: alone if it is a whole text, or the document is kept
    /// from the server or not sent yet; past the bound, the oldest goes.
    pub(super) fn give(&mut self, given: Given) {
        if given.splices.is_none() || self.closed.is_some() || !self.sent {
            self.given.clear();
        }
        self.given.push_back(given);
        while self.given.len() > KEPT_VERSIONS_MAX {
            self.given.pop_front();
        }
    }

    /// The latest text given went to the server as `version`.
    pub(super) fn went_as(&mut self, version: i32) {
        if let Some(latest) = self.given.back_mut() {
            latest.version = Some(version);
        }
    }
}

impl Documents {
    /// The diagnostics `report`, for the file at `path`, gives the document open with the
    /// server at that path, by server `server`: placed against the text of the version it
    /// names, or of the latest sent when it names none, then mapped through the splices of the
    /// texts given since, composed into one batch. A versioned report lets the texts before its
    /// version go.
    pub(in crate::server) fn diagnose(
        &mut self,
        path: &Path,
        report: &Published,
        server: &str,
        encoding: Encoding,
    ) -> Diagnose {
        let Some((&id, kept)) = self.0.iter_mut().find(|(_, kept)| {
            kept.sent && kept.closed.is_none() && kept.path.as_deref() == Some(path)
        }) else {
            return Diagnose::NotOpen;
        };
        let index = match report.version {
            Some(version) => kept.given.iter().position(|g| g.version == Some(version)),
            None => kept.given.iter().rposition(|g| g.version.is_some()),
        };
        let Some(index) = index else {
            return Diagnose::Stale;
        };
        let latest = kept.given.len() - 1;
        // The batches since, as one; `None` past a whole text, or if they do not compose.
        let batch = kept
            .given
            .range(index + 1..)
            .try_fold(Vec::new(), |batch, given| {
                compose(&batch, given.splices.as_deref()?)
            });
        let (from, batch) = match (batch, report.version) {
            (Some(batch), _) => (index, batch),
            (None, Some(_)) => return Diagnose::Stale,
            // A report for no version, past a whole text: placed against the latest.
            (None, None) => (latest, Vec::new()),
        };
        let text = &kept.given[from].text;
        let mut diagnostics = Diagnostics::new(
            server,
            report.diagnostics.iter().map(|reported| New {
                range: position::to_range(text, reported.start, reported.end, encoding),
                severity: reported.severity,
                tags: reported.tags,
                source: &reported.source,
                code: &reported.code,
                message: &reported.message,
            }),
            text.len(),
        );
        diagnostics.splice(&batch);
        let revision = kept.given[latest].revision;
        if report.version.is_some() {
            kept.given.drain(..index);
        }
        Diagnose::For {
            id,
            revision,
            diagnostics,
        }
    }

    #[cfg(test)]
    pub(in crate::server) fn given(&self, id: u64) -> usize {
        self.0.get(&id).map_or(0, |kept| kept.given.len())
    }
}
