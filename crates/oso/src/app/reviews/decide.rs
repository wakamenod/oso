//! A review's decisions (ADR 0122, "Decisions", step 5): accept stages hunks on the review's
//! thread (`stage`), reject reverts them in the file through its buffer and saves it (`revert`),
//! viewed marks them, and undo takes back the last of these. The person's word starts each; no
//! agent tool decides.
//!
//! Must not depend on: key presses or how a frame is drawn; the commands ask by
//! [`ReviewRequest`], and this answers.
//!
//! - Only the work tree's scopes take accept and reject (Q-B): a commit's, a branch's, a
//!   range's or a pull request's (step 12b) say so. In the staged scope, reject unstages, and
//!   accept says it is staged already.
//! - Only hunks of a modified text file that is no link are accepted or rejected (Q-E): an
//!   added, deleted, renamed, binary or linked file says why.
//! - Hunks are named by the model's reading they were picked in and their index there, shared,
//!   never copied; once the model holds a newer reading, each is found again by its text, the
//!   k-th of one text going to the k-th, as marks are carried (Q19).
//! - A hunk that leaves the review by its own decision, staged in the unstaged scope, unstaged
//!   in the staged one, or reverted, loses its whole mark, decision and viewed, so it leaves
//!   uncounted (Q-G). A rejected hunk always leaves, so a header never says "rejected".
//! - Undo refuses while a decision is still under way, and a failed undo keeps its decision to
//!   undo again. At most [`MAX_DONE`] decisions are kept, and patches of [`MAX_DONE_BYTES`] in all.

mod revert;
mod stage;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use oso_core::editor::{ReviewId, ReviewRequest};
use oso_core::review::{Decision, Hunk, Review, Shape};

use crate::app::App;
use crate::review::{Content, Entry, Reading, Scope, Status};

#[cfg(test)]
use revert::revert_edits;

/// The most decisions a review keeps for undo.
pub(super) const MAX_DONE: usize = 1000;
/// The most bytes of patches the decisions kept for undo hold.
const MAX_DONE_BYTES: usize = 64 * 1024 * 1024;
/// A link's mode, as git lists it.
const LINK: u32 = 0o120_000;

/// What a review keeps of its decisions.
#[derive(Default)]
pub(super) struct Decisions {
    /// What was decided, the last last.
    done: VecDeque<Done>,
    /// Stages asked of the review's thread, by ticket.
    staging: Vec<stage::Staging>,
    next_ticket: u64,
    /// Reverts waiting for their file's buffer to load or to be saved.
    reverting: Vec<revert::Reverting>,
    /// The latest listing, for each file's status and modes.
    pub(super) listing: Arc<[Entry]>,
    /// Each file's latest reading, whose new side a revert checks the lines around a hunk by.
    readings: HashMap<Box<[u8]>, Arc<Reading>>,
}

impl Decisions {
    /// Keeps `reading` as its file's latest, shared with the view.
    pub(super) fn keep_reading(&mut self, reading: &Arc<Reading>) {
        self.readings
            .insert(reading.entry.path.clone(), Arc::clone(reading));
    }

    /// The latest reading of the file at `path`.
    pub(super) fn reading(&self, path: &[u8]) -> Option<&Arc<Reading>> {
        self.readings.get(path)
    }

    /// Whether a decision is under way: staging, or reverting.
    fn pending(&self) -> bool {
        !self.staging.is_empty() || !self.reverting.is_empty()
    }
}

/// Hunks of a file, by the model's reading they were picked in and their index there.
#[derive(Clone)]
struct Picked {
    path: Box<[u8]>,
    shape: Arc<Shape>,
    hunks: Vec<usize>,
}

impl Picked {
    fn hunks(&self) -> impl Iterator<Item = &Hunk> {
        (self.hunks.iter()).filter_map(|at| self.shape.hunks().get(*at))
    }

    /// The same hunks in the model's reading now: by index while it is the same reading, else
    /// by their text, the k-th of one text to the k-th.
    fn now(&self, model: &Review) -> Vec<usize> {
        let Some(now) = model.file(&self.path) else {
            return Vec::new();
        };
        if Arc::ptr_eq(now, &self.shape) {
            return self.hunks.clone();
        }
        // One pass each way: the k-th hunk of one text then is the k-th of it now.
        let key = |hunk: &Hunk| (hunk.removed.clone(), hunk.added.clone());
        let mut found: HashMap<_, Vec<usize>> = HashMap::new();
        for (at, hunk) in now.hunks().iter().enumerate() {
            found.entry(key(hunk)).or_default().push(at);
        }
        let mut seen: HashMap<_, usize> = HashMap::new();
        let mut nth = Vec::with_capacity(self.shape.hunks().len());
        for hunk in self.shape.hunks() {
            let count = seen.entry(key(hunk)).or_default();
            nth.push(*count);
            *count += 1;
        }
        (self.hunks.iter())
            .filter_map(|at| {
                let hunk = self.shape.hunks().get(*at)?;
                found.get(&key(hunk))?.get(nth[*at]).copied()
            })
            .collect()
    }
}

/// What a decision does to its hunks' marks.
#[derive(Clone, Copy)]
enum Marking {
    /// The hunks stay shown: their decision is this, their viewed mark kept.
    Decided(Option<Decision>),
    /// The hunks leave the review by the decision: their whole mark goes, so they leave
    /// uncounted (Q-G).
    Leaves,
}

/// One decision, as undo takes it back.
enum Done {
    Staged {
        picked: Picked,
        patch: Arc<[u8]>,
        reverse: bool,
    },
    Reverted {
        picked: Picked,
        reading: Arc<Reading>,
    },
    Viewed {
        picked: Picked,
        viewed: bool,
    },
}

impl Done {
    fn bytes(&self) -> usize {
        match self {
            Done::Staged { patch, .. } => patch.len(),
            _ => 0,
        }
    }
}

/// What a scope lets accept and reject do: `true` for the work tree, `false` for the index.
fn work_tree(scope: &Scope) -> Result<bool, &'static str> {
    match scope {
        Scope::Commit { .. }
        | Scope::Branch { .. }
        | Scope::Range { .. }
        | Scope::PullRequest { .. } => Err(
            "Accept and reject change the work tree's changes: not in a commit's, branch's, range's or pull request's review",
        ),
        Scope::Staged => Ok(false),
        Scope::Session { .. } | Scope::Uncommitted | Scope::Unstaged => Ok(true),
    }
}

/// Why the file `entry` lists takes no accept or reject, if it does not.
fn refused(entry: Option<&Entry>) -> Option<&'static str> {
    let entry = entry?;
    if entry.old_mode & 0o170_000 == LINK || entry.new_mode & 0o170_000 == LINK {
        return Some("A link: decide on it in git");
    }
    match entry.status {
        Status::Modified => None,
        Status::Added => Some("A new file: M-x delete-file removes it"),
        Status::Deleted => Some("A deleted file: git restore brings it back"),
        Status::Renamed | Status::Copied => Some("A renamed file: decide on it in git"),
        _ => Some("Only a modified file's hunks are accepted or rejected"),
    }
}

/// `path` cut to one line of at most 80 characters, for a message.
fn named(path: &[u8]) -> String {
    let name = String::from_utf8_lossy(path);
    crate::commands::review::one_line(&name, crate::commands::review::MAX_NAME)
}

/// `n hunk` or `n hunks`.
fn hunks_said(count: usize) -> String {
    match count {
        1 => "1 hunk".to_owned(),
        n => format!("{n} hunks"),
    }
}

impl App {
    /// Carries out a decision a review command asked for.
    pub(super) fn decision_request(&mut self, request: ReviewRequest) {
        let (review, path, hunks, kind) = match request {
            ReviewRequest::Accept {
                review,
                path,
                hunks,
            } => (review, path, hunks, Decision::Accepted),
            ReviewRequest::Reject {
                review,
                path,
                hunks,
            } => (review, path, hunks, Decision::Rejected),
            ReviewRequest::Viewed {
                review,
                path,
                hunks,
            } => return self.view_hunks(review, path, &hunks),
            ReviewRequest::Undo(review) => return self.undo_decision(review),
            other => return log::debug!("not a decision: {other:?}"),
        };
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        let picked = match self.pick(index, path, &hunks, kind) {
            Ok(picked) => picked,
            Err(why) => return self.editor.message(why),
        };
        match (kind, work_tree(&self.reviews.open[index].scope)) {
            (_, Err(why)) => self.editor.message(why),
            (Decision::Accepted, Ok(false)) => self.editor.message("Already staged: x unstages it"),
            (Decision::Accepted, Ok(true)) => self.stage_picked(index, picked, false),
            (Decision::Rejected, Ok(false)) => self.stage_picked(index, picked, true),
            (Decision::Rejected, Ok(true)) => self.revert_picked(index, picked),
        }
    }

    /// Hunks `hunks` of the model's reading of `path` that `kind` may decide on, or why none.
    /// A file decided on has been looked at: its dropped decisions are no longer said.
    fn pick(
        &mut self,
        index: usize,
        path: Box<[u8]>,
        hunks: &[usize],
        kind: Decision,
    ) -> Result<Picked, &'static str> {
        let running = &self.reviews.open[index];
        if running.decisions.pending() {
            return Err("A decision is under way: try in a moment");
        }
        let entry = (running.decisions.listing.iter()).find(|entry| *entry.path == *path);
        if let Some(why) = refused(entry) {
            return Err(why);
        }
        let model = self
            .editor
            .review_mut(running.id)
            .ok_or("Not in a review")?;
        model.clear_dropped(&path);
        let shape = Arc::clone(
            model
                .file(&path)
                .ok_or("Not shown as text: nothing to decide")?,
        );
        // Accepting what is accepted already would stage it twice: those are left out.
        let hunks: Vec<usize> = (hunks.iter().copied())
            .filter(|at| *at < shape.hunks().len())
            .filter(|at| {
                kind != Decision::Accepted || model.decision(&path, *at) != Some(Decision::Accepted)
            })
            .collect();
        if hunks.is_empty() {
            return Err(if kind == Decision::Accepted {
                "Accepted already"
            } else {
                "Not on a hunk"
            });
        }
        Ok(Picked { path, shape, hunks })
    }

    /// Marks `picked`'s hunks in the model's reading now as `marking` says.
    fn mark_picked(&mut self, index: usize, picked: &Picked, marking: Marking) {
        let id = self.reviews.open[index].id;
        let Some(model) = self.editor.review_mut(id) else {
            return;
        };
        for at in picked.now(model) {
            match marking {
                Marking::Decided(decision) => {
                    model.decide(&picked.path, at, decision);
                }
                Marking::Leaves => {
                    model.decide(&picked.path, at, None);
                    model.set_viewed(&picked.path, at, false);
                }
            }
        }
        self.tell_cards(index, &picked.path);
    }

    /// Marks hunks `hunks` of `path` viewed, or clears the mark when each has it.
    fn view_hunks(&mut self, review: ReviewId, path: Box<[u8]>, hunks: &[usize]) {
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        let Some(model) = self.editor.review_mut(review) else {
            return;
        };
        model.clear_dropped(&path);
        let Some(shape) = model.file(&path).cloned() else {
            return self.editor.message("Not shown as text: nothing to decide");
        };
        let viewed = !hunks.iter().all(|at| model.viewed(&path, *at));
        // Only those it changes, so that undo puts back no mark it did not set.
        let changed: Vec<usize> = (hunks.iter().copied())
            .filter(|at| *at < shape.hunks().len() && model.viewed(&path, *at) != viewed)
            .collect();
        for at in &changed {
            model.set_viewed(&path, *at, viewed);
        }
        if changed.is_empty() {
            return self.editor.message("Not on a hunk");
        }
        let picked = Picked {
            path,
            shape,
            hunks: changed,
        };
        self.tell_cards(index, &picked.path);
        self.keep_done(index, Done::Viewed { picked, viewed });
    }

    /// Takes back the review's last decision, unless one is under way.
    fn undo_decision(&mut self, review: ReviewId) {
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        let decisions = &mut self.reviews.open[index].decisions;
        if decisions.pending() {
            return self
                .editor
                .message("A decision is under way: undo once it is done");
        }
        let Some(done) = decisions.done.pop_back() else {
            return self.editor.message("No decision to undo");
        };
        match done {
            Done::Staged {
                picked,
                patch,
                reverse,
            } => self.unstage_done(index, picked, patch, reverse),
            Done::Reverted { picked, reading } => self.put_back(index, picked, reading),
            Done::Viewed { picked, viewed } => {
                if let Some(model) = self.editor.review_mut(review) {
                    for at in picked.now(model) {
                        model.set_viewed(&picked.path, at, !viewed);
                    }
                }
                self.editor.message("Viewed mark undone");
                self.tell_cards(index, &picked.path);
            }
        }
    }

    /// Keeps `done` to undo, dropping the oldest past [`MAX_DONE`] decisions or
    /// [`MAX_DONE_BYTES`] of patches.
    fn keep_done(&mut self, index: usize, done: Done) {
        let kept = &mut self.reviews.open[index].decisions.done;
        kept.push_back(done);
        let mut bytes: usize = kept.iter().map(Done::bytes).sum();
        while kept.len() > MAX_DONE || (bytes > MAX_DONE_BYTES && kept.len() > 1) {
            if let Some(gone) = kept.pop_front() {
                bytes -= gone.bytes();
            }
        }
    }

    /// Puts `done` back to undo, as its undo failed.
    fn keep_undone(&mut self, index: usize, done: Done) {
        self.reviews.open[index].decisions.done.push_back(done);
    }
}

/// The new side of a text reading.
fn new_side(reading: &Reading) -> Option<&[u8]> {
    match &reading.content {
        Content::Text { new, .. } => Some(new),
        _ => None,
    }
}

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
