//! A buffer's highlights, and the batches of edits since recent revisions, for the syntax
//! worker's jobs and reports (ADR 0111). An edit maps the highlights on the clones it is made
//! on (ADR 0053), and an undo staged over several turns slice by slice on its own copy
//! (ADR 0119). A report made for an older revision is mapped through the batches since.
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
    /// How many times the highlights were taken from a report or dropped, ever, so that an
    /// undo staged meanwhile takes them too.
    changes: u64,
}

/// Highlighting followed a slice at a time on a copy, as an undo staged over several turns
/// follows it (ADR 0119).
#[derive(Debug)]
pub(super) struct FollowedHighlighting {
    highlights: Option<Highlights>,
    /// The slices' batches composed, or `None` once they do not compose or pass
    /// [`LOG_SPLICES`] (`too_large`).
    batch: Option<Vec<Splice>>,
    too_large: bool,
    /// The buffer's count of changes as it began.
    changes: u64,
}

impl FollowedHighlighting {
    /// How many splices the log's batch holds, which each batch more is composed with.
    pub(super) fn composed(&self) -> usize {
        self.batch.as_ref().map_or(0, Vec::len)
    }

    /// Maps the copy through one more batch of edits, and composes it into the log's batch.
    pub(super) fn follow(&mut self, step: &[OffsetEdit]) {
        let step: Vec<Splice> = step.iter().map(splice_of).collect();
        if let Some(highlights) = &mut self.highlights {
            highlights.splice(&step);
        }
        if self.too_large {
            return;
        }
        self.batch = self.batch.take().and_then(|batch| compose(&batch, &step));
        if self
            .batch
            .as_ref()
            .is_some_and(|batch| batch.len() > LOG_SPLICES)
        {
            (self.batch, self.too_large) = (None, true);
        }
    }

    /// What it all does to `now`, the buffer's highlighting, to commit, if the buffer can be
    /// highlighted afterwards (`keep`). Highlights a report brought meanwhile are taken too,
    /// mapped through the batch; past the log, the copy stays, as when the report comes after.
    pub(super) fn staged(self, now: &Highlighting, keep: bool) -> StagedHighlighting {
        if !keep {
            return StagedHighlighting {
                batch: None,
                highlights: None,
            };
        }
        let highlights = if now.changes == self.changes {
            self.highlights
        } else {
            match (&now.highlights, &self.batch) {
                (None, _) => None,
                (Some(highlights), Some(batch)) => {
                    let mut highlights = highlights.clone();
                    highlights.splice(batch);
                    Some(highlights)
                }
                (Some(_), None) => self.highlights,
            }
        };
        StagedHighlighting {
            highlights: highlights.filter(|_| self.batch.is_some() || self.too_large),
            batch: self.batch,
        }
    }
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
    /// and no highlights. A batch past [`LOG_SPLICES`], which the log could not hold, stops
    /// composing and leaves no batch, but the highlights follow it.
    fn stage(&self, steps: &[Vec<OffsetEdit>], keep: bool) -> StagedHighlighting {
        let mut followed = self.follow(keep);
        if keep {
            steps.iter().for_each(|step| followed.follow(step));
        }
        followed.staged(self, keep)
    }

    /// A copy to follow edits on, slice by slice, if the buffer can be highlighted (`may`).
    fn follow(&self, may: bool) -> FollowedHighlighting {
        FollowedHighlighting {
            highlights: self.highlights.clone().filter(|_| may),
            batch: may.then(Vec::new),
            too_large: false,
            changes: self.changes,
        }
    }

    /// Puts a staged change in, as the content becomes `revision`, and returns the highlights it
    /// replaced, for the caller to free. Moves only, and cannot panic.
    pub(super) fn commit(
        &mut self,
        revision: u64,
        staged: StagedHighlighting,
    ) -> Option<Highlights> {
        let old = std::mem::replace(&mut self.highlights, staged.highlights);
        let Some(batch) = staged.batch else {
            self.forget_log();
            return old;
        };
        self.logged += batch.len();
        self.log.push_back((revision, batch));
        while self.log.len() > LOG_REVISIONS || self.logged > LOG_SPLICES {
            let Some((_, oldest)) = self.log.pop_front() else {
                break;
            };
            self.logged -= oldest.len();
        }
        old
    }

    /// Drops the highlights and the log, as for a buffer that can no longer be highlighted.
    pub(super) fn forget(&mut self) {
        self.highlights = None;
        self.changes += 1;
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
    /// Whether the syntax worker may have the buffer now: as [`Self::highlightable`] says of
    /// its text as it is. A buffer that may hold secrets never may, nor one still unjudged.
    pub fn can_highlight(&self) -> bool {
        self.highlightable(self.text.len())
    }

    /// Whether the buffer can be highlighted with a text of `len` bytes: a language mode, not an
    /// internal buffer, no larger than [`MAX_BYTES`], and not one that may hold secrets, which is
    /// never parsed (ADR 0111). Any other keeps no highlights and no log.
    fn highlightable(&self, len: usize) -> bool {
        self.may_highlight() && len <= MAX_BYTES
    }

    /// Whether the buffer can be highlighted at some length: a language mode, not an internal
    /// buffer, and not one that may hold secrets.
    fn may_highlight(&self) -> bool {
        let language = match self.mode {
            Mode::Rust | Mode::Markdown | Mode::Toml => true,
            Mode::Fundamental | Mode::CopyList | Mode::CopyView | Mode::Sessions => false,
        };
        language && !self.internal && !self.is_secret()
    }

    /// A copy of the highlighting to follow an undo staged over several turns (ADR 0119).
    pub(super) fn follow_highlighting(&self) -> FollowedHighlighting {
        self.highlighting.follow(self.may_highlight())
    }

    /// What a crossing that followed `followed` and leaves `len` bytes does to highlighting.
    pub(super) fn stage_followed(
        &self,
        followed: FollowedHighlighting,
        len: usize,
    ) -> StagedHighlighting {
        followed.staged(&self.highlighting, self.highlightable(len))
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
        self.highlighting.changes += 1;
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
        self.highlighting.changes += 1;
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
