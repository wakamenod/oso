//! A buffer's highlights, and the batches of edits since recent revisions, for the syntax
//! worker's jobs and reports (ADR 0111). An edit maps the highlights on the clones it is made
//! on (ADR 0053), and a report made for an older revision is mapped through the batches since.
//!
//! Must not depend on: `window`, `editor` or `command`.

use std::collections::VecDeque;

use super::Buffer;
use crate::selection::OffsetEdit;
use crate::syntax::{Highlights, Splice, compose};

/// How many revisions' batches a buffer keeps. A report or a job older than that starts over.
const LOG_REVISIONS: usize = 64;

/// What highlighting keeps on a buffer.
#[derive(Clone, Debug, Default)]
pub(super) struct Highlighting {
    highlights: Option<Highlights>,
    /// Each recent revision with the batch that made it, oldest first.
    log: VecDeque<(u64, Vec<Splice>)>,
}

/// What an edit, an undo or a redo made on clones does to highlighting, to commit with it.
#[derive(Debug)]
pub(crate) struct StagedHighlighting {
    batch: Option<Vec<Splice>>,
    highlights: Option<Highlights>,
}

impl Highlighting {
    /// Maps the highlights through `steps`, batch by batch as they applied, on a clone. Steps that
    /// do not compose leave no batch, and no highlights.
    pub(super) fn stage(&self, steps: &[Vec<OffsetEdit>]) -> StagedHighlighting {
        let mut batch = Some(Vec::new());
        let mut highlights = self.highlights.clone();
        for step in steps {
            let step: Vec<Splice> = step.iter().map(splice_of).collect();
            if let Some(highlights) = &mut highlights {
                highlights.splice(&step);
            }
            batch = batch.and_then(|batch| compose(&batch, &step));
        }
        StagedHighlighting {
            highlights: highlights.filter(|_| batch.is_some()),
            batch,
        }
    }

    /// Puts a staged change in, as the content becomes `revision`. Cannot panic.
    pub(super) fn commit(&mut self, revision: u64, staged: StagedHighlighting) {
        self.highlights = staged.highlights;
        match staged.batch {
            Some(batch) => {
                if self.log.len() == LOG_REVISIONS {
                    self.log.pop_front();
                }
                self.log.push_back((revision, batch));
            }
            None => self.log.clear(),
        }
    }

    /// The batch from `revision` to `now`, or `None` if the log no longer reaches it.
    fn since(&self, revision: u64, now: u64) -> Option<Vec<Splice>> {
        if revision == now {
            return Some(Vec::new());
        }
        let first = self
            .log
            .iter()
            .position(|(made, _)| *made == revision + 1)?;
        self.log
            .range(first..)
            .try_fold(Vec::new(), |batch, (_, next)| compose(&batch, next))
    }
}

fn splice_of(edit: &OffsetEdit) -> Splice {
    Splice {
        start: edit.start,
        old_len: edit.old_len,
        new_len: edit.new_len,
    }
}

impl Buffer {
    /// What the text is, run by run, as the syntax worker last reported and the edits since
    /// guessed, or `None` if the buffer is not highlighted (ADR 0111).
    pub fn highlights(&self) -> Option<&Highlights> {
        // A buffer that may hold secrets is never parsed, and shows none it may have had.
        self.highlighting
            .highlights
            .as_ref()
            .filter(|_| !self.is_secret())
    }

    /// Takes highlights the worker made for `revision`, mapped through the edits since. Returns
    /// false, and keeps what it had, if the buffer may hold secrets, the log no longer reaches
    /// that revision, or the highlights do not cover the text.
    pub fn take_highlights(&mut self, revision: u64, mut highlights: Highlights) -> bool {
        if self.is_secret() {
            return false;
        }
        let Some(batch) = self.highlighting.since(revision, self.revision) else {
            return false;
        };
        highlights.splice(&batch);
        if highlights.len() != self.text.len() {
            return false;
        }
        self.highlighting.highlights = Some(highlights);
        true
    }

    /// Stops showing highlights, as when the buffer may now hold secrets or changes mode.
    pub fn drop_highlights(&mut self) {
        self.highlighting.highlights = None;
    }

    /// One batch that takes the text at `revision` to the text now, for the worker's parse, or
    /// `None` if the log no longer reaches it and the worker must parse from nothing.
    pub fn splices_since(&self, revision: u64) -> Option<Vec<Splice>> {
        self.highlighting.since(revision, self.revision)
    }
}

#[cfg(test)]
mod tests;
