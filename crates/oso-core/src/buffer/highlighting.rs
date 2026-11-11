//! A buffer's highlights, and the batches of edits since recent revisions, for the syntax
//! worker's jobs and reports (ADR 0111). An edit maps the highlights on the clones it is made
//! on (ADR 0053), and a report made for an older revision is mapped through the batches since.
//!
//! Must not depend on: `window`, `editor` or `command`.

use std::collections::VecDeque;

use super::{Buffer, Mode};
use crate::selection::OffsetEdit;
use crate::syntax::{Highlights, MAX_BYTES, Splice, compose};

/// How many revisions' batches a buffer keeps. A report or a job older than that starts over.
const LOG_REVISIONS: usize = 64;

/// How many splices the log holds in all, so that a few commands at many cursors cannot make
/// it large. The oldest batches go first.
const LOG_SPLICES: usize = 4096;

/// What highlighting keeps on a buffer.
#[derive(Clone, Debug, Default)]
pub(super) struct Highlighting {
    highlights: Option<Highlights>,
    /// Each recent revision with the batch that made it, oldest first.
    log: VecDeque<(u64, Vec<Splice>)>,
    /// The splices in `log`.
    logged: usize,
}

/// What an edit, an undo or a redo made on clones does to highlighting, to commit with it.
#[derive(Debug)]
pub(crate) struct StagedHighlighting {
    /// The edit's batch, or `None` to keep no log: the buffer cannot be highlighted, or the
    /// steps did not compose.
    batch: Option<Vec<Splice>>,
    highlights: Option<Highlights>,
}

impl Highlighting {
    /// Maps the highlights through `steps`, batch by batch as they applied, on a clone, if the
    /// buffer can be highlighted afterwards (`keep`). Steps that do not compose leave no batch,
    /// and no highlights.
    fn stage(&self, steps: &[Vec<OffsetEdit>], keep: bool) -> StagedHighlighting {
        if !keep {
            return StagedHighlighting {
                batch: None,
                highlights: None,
            };
        }
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

    /// Puts a staged change in, as the content becomes `revision`. Moves only, and cannot panic.
    pub(super) fn commit(&mut self, revision: u64, staged: StagedHighlighting) {
        self.highlights = staged.highlights;
        let Some(batch) = staged.batch else {
            self.forget_log();
            return;
        };
        self.logged += batch.len();
        self.log.push_back((revision, batch));
        while self.log.len() > LOG_REVISIONS || self.logged > LOG_SPLICES {
            let Some((_, oldest)) = self.log.pop_front() else {
                break;
            };
            self.logged -= oldest.len();
        }
    }

    /// Drops the highlights and the log, as for a buffer that can no longer be highlighted.
    pub(super) fn forget(&mut self) {
        self.highlights = None;
        self.forget_log();
    }

    /// Drops the log, so that no report or job from before now is followed.
    pub(super) fn forget_log(&mut self) {
        self.log.clear();
        self.logged = 0;
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
    /// Whether the buffer can be highlighted with a text of `len` bytes: a language mode, not an
    /// internal buffer, no larger than [`MAX_BYTES`], and not one that may hold secrets, which is
    /// never parsed (ADR 0111). Any other keeps no highlights and no log.
    fn highlightable(&self, len: usize) -> bool {
        let language = match self.mode {
            Mode::Rust | Mode::Markdown | Mode::Toml => true,
            Mode::Fundamental | Mode::CopyList | Mode::CopyView => false,
        };
        language && !self.internal && len <= MAX_BYTES && !self.is_secret()
    }

    /// What an edit's steps, which leave `len` bytes, do to highlighting, made on clones while
    /// the edit is staged.
    pub(super) fn stage_highlighting(
        &self,
        steps: &[Vec<OffsetEdit>],
        len: usize,
    ) -> StagedHighlighting {
        self.highlighting.stage(steps, self.highlightable(len))
    }

    /// Drops the highlights and the log if the buffer can no longer be highlighted, after its
    /// verdict, its mode or its text changed otherwise than by an edit.
    pub(super) fn recheck_highlighting(&mut self) {
        if !self.highlightable(self.text.len()) {
            self.highlighting.forget();
        }
    }

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
    /// false, and keeps what it had, if the buffer cannot be highlighted, the log no longer
    /// reaches that revision, or the highlights do not cover the text.
    pub fn take_highlights(&mut self, revision: u64, mut highlights: Highlights) -> bool {
        if !self.highlightable(self.text.len()) {
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

    /// Whether it keeps highlights or a log at all, whatever it shows.
    #[cfg(test)]
    pub(crate) fn keeps_highlighting(&self) -> bool {
        self.highlighting.highlights.is_some() || !self.highlighting.log.is_empty()
    }

    /// Stops showing highlights, as when highlighting is turned off.
    pub fn drop_highlights(&mut self) {
        self.highlighting.highlights = None;
    }

    /// One batch that takes the text at `revision` to the text now, for the worker's parse, or
    /// `None` if the log no longer reaches it, the buffer cannot be highlighted, or a load or a
    /// reload came since, so that the worker must parse from nothing.
    pub fn splices_since(&self, revision: u64) -> Option<Vec<Splice>> {
        if !self.highlightable(self.text.len()) {
            return None;
        }
        self.highlighting.since(revision, self.revision)
    }
}

#[cfg(test)]
mod tests;
