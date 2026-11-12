//! What the schedule no longer wants, for a thread to free once it has unlocked: documents,
//! whose trees cost as much as their nodes to free, and the texts that waited in a slot that
//! went, which the schedule may have been the last to hold.
//!
//! Must not depend on: threads or the lock; it only holds what it is given until dropped.

use oso_core::rope::Rope;

use super::Waiting;
use crate::document::Document;

/// Documents and texts to free, from [`super::Scheduler::take_doomed`]: dropping it frees them,
/// which a thread does with nothing locked.
pub struct Doomed {
    documents: Vec<Document>,
    texts: Vec<Rope>,
}

impl Doomed {
    pub(super) fn new() -> Self {
        Self {
            documents: Vec::new(),
            texts: Vec::new(),
        }
    }

    /// Whether there is nothing to free.
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty() && self.texts.is_empty()
    }

    pub(super) fn push(&mut self, document: Document) {
        self.documents.push(document);
    }

    pub(super) fn extend(&mut self, documents: impl IntoIterator<Item = Document>) {
        self.documents.extend(documents);
    }

    /// Keeps the text that waited in a slot that went, so that it is not freed under the lock.
    pub(super) fn bury(&mut self, waiting: Waiting) {
        match waiting {
            Waiting::Nothing => {}
            Waiting::Edits { text, .. } | Waiting::Reset { text, .. } => self.texts.push(text),
        }
    }

    /// How many documents it holds.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.documents.len()
    }

    /// How many texts it holds.
    #[cfg(test)]
    pub(crate) fn texts(&self) -> usize {
        self.texts.len()
    }
}
