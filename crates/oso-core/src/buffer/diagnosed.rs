//! A buffer's diagnostics, as the language server that follows it last reported them (ADR 0138,
//! Diagnostics, step 6): taken in mapped through the edits since the revision they were made
//! for, then following each edit in place as it commits.
//!
//! Must not depend on: `editor` or `window`; the host hands over each set.
//!
//! - Only a buffer a server follows, and that may not hold secrets, has or takes a set; a load,
//!   unfollowing, or a verdict that it may hold secrets drops it.
//! - An edit, an undo, a reload and a recovery are one step, which the set follows in place at
//!   commit, O(entries × log splices), however many splices the step has. A sliced undo's
//!   crossing follows its composed batch; one past the log's bound drops the set until the
//!   next report.

use super::Buffer;
use super::highlighting::splice_of;
use crate::diagnostic::Diagnostics;
use crate::selection::OffsetEdit;
use crate::syntax::Splice;

#[cfg(test)]
mod tests;

impl Buffer {
    /// What the server that follows it last reported, mapped through the edits since, or `None`
    /// while none follows it or it may hold secrets. A set with no entries is a server's report
    /// of none.
    pub fn diagnostics(&self) -> Option<&Diagnostics> {
        self.diagnosed.as_ref().filter(|_| self.followed_now())
    }

    /// Takes the diagnostics a server reported for `revision`, mapped through the edits since,
    /// in place of those it had. Returns false, and keeps what it had, if no server follows it,
    /// the log no longer reaches that revision, or the set does not fit the text: those it had
    /// followed the edits, and the next report puts them right.
    pub fn take_diagnostics(&mut self, revision: u64, mut diagnostics: Diagnostics) -> bool {
        if !self.followed_now() {
            return false;
        }
        let Some(batch) = self.highlighting.since(revision, self.revision) else {
            return false;
        };
        diagnostics.splice(&batch);
        if !diagnostics.fits(self.text.len()) {
            return false;
        }
        self.diagnosed = Some(diagnostics);
        true
    }

    /// The set follows an edit as it commits: through `batch` when the log kept one, else
    /// through `step`, the edit's one step; with neither, as after a crossing past the log's
    /// bound, it goes.
    pub(super) fn follow_diagnostics(&mut self, batch: Option<&[Splice]>, step: &[OffsetEdit]) {
        let Some(diagnostics) = &mut self.diagnosed else {
            return;
        };
        match batch {
            Some(batch) => diagnostics.splice(batch),
            None if !step.is_empty() => {
                let step: Vec<Splice> = step.iter().map(splice_of).collect();
                diagnostics.splice(&step);
            }
            None => self.diagnosed = None,
        }
    }
}
