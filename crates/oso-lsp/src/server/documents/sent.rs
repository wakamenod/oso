//! The documents as the server has them, for an ask about one and to place an answer's
//! ranges (ADR 0138, step 8): only a document open with the server, with no whole text waiting,
//! has the text the server has.
//!
//! Must not depend on: the session's loop, the pipes or the outbox.

use std::collections::HashMap;
use std::path::Path;

use oso_core::rope::Rope;

use super::{Documents, Kept, Waits};

impl Kept {
    /// The caller's revision of the text the server has, if it has this document's text.
    fn sent_revision(&self) -> Option<u64> {
        let has = self.sent && self.waits == Waits::Nothing && self.closed.is_none();
        has.then(|| self.given.back().map(|given| given.revision))
            .flatten()
    }
}

impl Documents {
    /// The URI and text of document `id` as the server has them, if they are those of the
    /// caller's revision `revision`.
    pub(in crate::server) fn as_sent(&self, id: u64, revision: u64) -> Option<(&str, &Rope)> {
        let kept = self.0.get(&id)?;
        (kept.sent_revision() == Some(revision)).then_some((kept.uri.as_str(), &kept.text))
    }

    /// The caller's revision of the text the server has of document `id`, and that text, if it
    /// has it (step 9: a completion's ranges are placed against it).
    pub(in crate::server) fn sent(&self, id: u64) -> Option<(u64, &Rope)> {
        let kept = self.0.get(&id)?;
        Some((kept.sent_revision()?, &kept.text))
    }

    /// The documents whose text the server has, by path: each one's id, the caller's revision
    /// of that text, and the text.
    pub(in crate::server) fn sent_by_path(&self) -> HashMap<&Path, (u64, u64, &Rope)> {
        self.0
            .iter()
            .filter_map(|(id, kept)| {
                let revision = kept.sent_revision()?;
                Some((kept.path.as_deref()?, (*id, revision, &kept.text)))
            })
            .collect()
    }
}
