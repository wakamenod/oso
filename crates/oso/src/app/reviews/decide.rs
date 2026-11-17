//! A review's decisions (ADR 0122, "Decisions", step 5): accept stages hunks on the review's
//! thread, reject reverts them in the file through its buffer and saves it safely, viewed marks
//! them, and undo takes back the last of these. The person's word starts each; no agent tool
//! decides.
//!
//! Must not depend on: key presses or how a frame is drawn; the commands ask by
//! [`ReviewRequest`](oso_core::editor::ReviewRequest), and this answers.
//!
//! - Only the work tree's scopes take accept and reject (Q-B): a commit's, a branch's or a
//!   range's say so. In the staged scope, reject unstages, and accept says it is staged already.
//! - Only hunks of a modified text file are accepted or rejected (Q-E): an added, deleted,
//!   renamed or binary file says why.
//! - A reject checks each hunk's new lines are where the reading put them in the file's buffer,
//!   which it opens unshown and keeps open if it was not (Q-D), and replaces them with the old
//!   lines as one undo step, then saves. A buffer with unsaved edits is asked about by the
//!   command first (Q-C).
//! - A hunk that leaves the review by its own decision, staged in the unstaged scope, unstaged
//!   in the staged one, or reverted, is not marked, so it is not counted as changed (Q-G).
//! - Undo applies the same patch the other way, or the reverse edit, checked the same way, or
//!   clears the viewed mark. At most [`MAX_DONE`] decisions are kept.

use std::collections::VecDeque;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use oso_core::buffer::{BufferId, Replacement};
use oso_core::editor::{ReviewId, ReviewRequest};
use oso_core::review::{Base, Decision, Hunk, Review};
use oso_core::rope::Rope;

use crate::app::App;
use crate::git::Id;
use crate::review::Status;
use crate::review::stage::{Apply, CHANGED, Stage, Staged};
use crate::review::{Entry, Request, Scope};

/// The most decisions a review keeps for undo.
pub(super) const MAX_DONE: usize = 1000;

/// What a review keeps of its decisions.
#[derive(Default)]
pub(super) struct Decisions {
    /// What was decided, the last last.
    done: VecDeque<Done>,
    /// Stages asked of the review's thread, in the order it answers them.
    staging: VecDeque<Staging>,
    next_serial: u64,
    /// Reverts waiting for their file's buffer to load or to be saved.
    reverting: Vec<Reverting>,
    /// The latest listing, for each file's status.
    pub(super) entries: Arc<[Entry]>,
}

/// One decision, as undo takes it back.
enum Done {
    Staged {
        path: Box<[u8]>,
        patch: Arc<[u8]>,
        reverse: bool,
        hunks: Vec<Hunk>,
    },
    Reverted {
        path: Box<[u8]>,
        hunks: Vec<Hunk>,
    },
    Viewed {
        path: Box<[u8]>,
        hunks: Vec<Hunk>,
        viewed: bool,
    },
}

/// A stage asked of the review's thread.
struct Staging {
    serial: u64,
    path: Box<[u8]>,
    hunks: Vec<Hunk>,
    reverse: bool,
    /// Whether it is an undo, which keeps no decision.
    undo: bool,
}

/// A revert of hunks in a file's buffer, waiting for the buffer.
struct Reverting {
    review: ReviewId,
    path: Box<[u8]>,
    buffer: BufferId,
    hunks: Vec<Hunk>,
    undo: bool,
    /// Whether its edit is made and its save waited for.
    saving: bool,
}

/// What a scope lets a decision do with the index or the work tree.
fn work_tree(scope: &Scope) -> Result<bool, &'static str> {
    match scope {
        Scope::Commit { .. } | Scope::Branch { .. } | Scope::Range { .. } => Err(
            "Accept and reject change the work tree's changes: not in a commit's, branch's or range's review",
        ),
        Scope::Staged => Ok(false),
        Scope::Session { .. } | Scope::Uncommitted | Scope::Unstaged => Ok(true),
    }
}

/// Why the file `entry` lists takes no decision, if it does not.
fn refused(entry: Option<&Entry>) -> Option<&'static str> {
    match entry.map(|entry| entry.status) {
        Some(Status::Modified) => None,
        Some(Status::Added) => Some("A new file: M-x delete-file removes it"),
        Some(Status::Deleted) => Some("A deleted file: git restore brings it back"),
        Some(Status::Renamed | Status::Copied) => Some("A renamed file: decide on it in git"),
        _ => Some("Only a modified file's hunks are accepted or rejected"),
    }
}

/// The edits that put `want` lines back as `put` lines for each of `hunks` in `text`, each at
/// the line its new start names, moved by what the hunks before it changed when `undone`, as a
/// revert put the old lines there. Descending, as edits apply.
///
/// # Errors
///
/// [`CHANGED`] when a hunk's lines are not there.
pub(super) fn revert_edits(
    text: &Rope,
    hunks: &[Hunk],
    undone: bool,
) -> Result<Vec<(Range<usize>, String)>, &'static str> {
    let mut hunks: Vec<&Hunk> = hunks.iter().collect();
    hunks.sort_by_key(|hunk| hunk.new_start);
    let joined = |lines: &[Box<[u8]>]| -> Result<String, &'static str> {
        let bytes: Vec<u8> = lines.concat();
        String::from_utf8(bytes).map_err(|_| CHANGED)
    };
    let lines = text.max_point().row + 1;
    let mut shift: i64 = 0;
    let mut edits = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        let (want, put) = if undone {
            (&hunk.removed, &hunk.added)
        } else {
            (&hunk.added, &hunk.removed)
        };
        let line = i64::from(hunk.new_start).saturating_sub(1) + shift;
        let start = match usize::try_from(line) {
            Ok(line) if line < lines => text.line_start(line),
            _ => return Err(CHANGED),
        };
        let want = joined(want)?;
        let end = start + want.len();
        let there: String = if end <= text.len() {
            text.chunks_in_range(start..end).collect()
        } else {
            String::new()
        };
        if there != want {
            return Err(CHANGED);
        }
        if undone {
            let (gone, came) = (hunk.added.len(), hunk.removed.len());
            shift += i64::try_from(came).unwrap_or(0) - i64::try_from(gone).unwrap_or(0);
        }
        edits.push((start..end, joined(put)?));
    }
    edits.reverse();
    Ok(edits)
}

/// The index in `model`'s reading of `path` of the hunk with `hunk`'s text.
fn hunk_now(model: &Review, path: &[u8], hunk: &Hunk) -> Option<usize> {
    let shape = model.file(path)?;
    (shape.hunks().iter()).position(|now| now.removed == hunk.removed && now.added == hunk.added)
}

/// The path of the file at `path` from the top of `root`.
fn file_at(root: &std::path::Path, path: &[u8]) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(root.join(std::ffi::OsStr::from_bytes(path)))
    }
    #[cfg(not(unix))]
    {
        Some(root.join(std::str::from_utf8(path).ok()?))
    }
}

impl App {
    /// Carries out a decision a review command asked for.
    pub(super) fn decision_request(&mut self, request: ReviewRequest) {
        // A file decided on has been looked at: its dropped decisions are no longer said.
        let looked = match &request {
            ReviewRequest::Accept { review, path, .. } => Some((*review, path)),
            ReviewRequest::Reject { review, path, .. } => Some((*review, path)),
            ReviewRequest::Viewed { review, path, .. } => Some((*review, path)),
            _ => None,
        };
        if let Some((review, path)) = looked
            && let Some(model) = self.editor.review_mut(review)
        {
            model.clear_dropped(path);
        }
        match request {
            ReviewRequest::Accept {
                review,
                path,
                hunks,
            } => self.accept_hunks(review, &path, &hunks),
            ReviewRequest::Reject {
                review,
                path,
                hunks,
            } => self.reject_hunks(review, &path, &hunks),
            ReviewRequest::Viewed {
                review,
                path,
                hunks,
            } => self.view_hunks(review, &path, &hunks),
            ReviewRequest::Undo(review) => self.undo_decision(review),
            other => log::debug!("not a decision: {other:?}"),
        }
    }

    /// Accepts hunks `hunks` of `path`: stages them, or says why not.
    pub(super) fn accept_hunks(&mut self, review: ReviewId, path: &[u8], hunks: &[usize]) {
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        match work_tree(&self.reviews.open[index].scope) {
            Err(why) => self.editor.message(why),
            Ok(false) => self.editor.message("Already staged: x unstages it"),
            Ok(true) => self.stage_hunks(index, path, hunks, false),
        }
    }

    /// Rejects hunks `hunks` of `path`: reverts them in the file, or unstages them in the staged
    /// scope, or says why not.
    pub(super) fn reject_hunks(&mut self, review: ReviewId, path: &[u8], hunks: &[usize]) {
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        match work_tree(&self.reviews.open[index].scope) {
            Err(why) => self.editor.message(why),
            Ok(false) => self.stage_hunks(index, path, hunks, true),
            Ok(true) => self.revert_hunks(index, path, hunks),
        }
    }

    /// The hunks `hunks` of the model's reading of `path`, and its old side's blob, or why they
    /// take no decision.
    fn decided_hunks(
        &self,
        index: usize,
        path: &[u8],
        hunks: &[usize],
    ) -> Result<(Id, Vec<Hunk>), &'static str> {
        let running = &self.reviews.open[index];
        let decisions = &running.decisions;
        let entry = decisions.entries.iter().find(|entry| *entry.path == *path);
        if let Some(why) = refused(entry) {
            return Err(why);
        }
        let shape = (self.editor.review(running.id))
            .and_then(|model| model.file(path))
            .ok_or("Not shown as text: nothing to decide")?;
        let Base::Blob(old) = shape.base() else {
            return Err("A new file: M-x delete-file removes it");
        };
        let old = Id::parse(old).ok_or("Not shown as text: nothing to decide")?;
        let picked = (hunks.iter())
            .filter_map(|at| shape.hunks().get(*at).cloned())
            .collect::<Vec<_>>();
        if picked.is_empty() {
            return Err("Not on a hunk");
        }
        Ok((old, picked))
    }

    /// Asks the review's thread to stage `hunks` of `path`, or unstage them with `reverse`.
    fn stage_hunks(&mut self, index: usize, path: &[u8], hunks: &[usize], reverse: bool) {
        let (old, hunks) = match self.decided_hunks(index, path, hunks) {
            Ok(found) => found,
            Err(why) => return self.editor.message(why),
        };
        let apply = Apply::Hunks {
            old,
            hunks: hunks.clone(),
        };
        self.ask_stage(index, path, apply, hunks, reverse, false);
    }

    fn ask_stage(
        &mut self,
        index: usize,
        path: &[u8],
        apply: Apply,
        hunks: Vec<Hunk>,
        reverse: bool,
        undo: bool,
    ) {
        let running = &mut self.reviews.open[index];
        let Some(reviewer) = &running.reviewer else {
            return self
                .editor
                .message("The review is not reading: g starts it again");
        };
        let decisions = &mut running.decisions;
        let serial = decisions.next_serial;
        decisions.next_serial += 1;
        let stage = Stage {
            serial,
            path: path.into(),
            apply,
            reverse,
        };
        if !reviewer.ask(Request::Stage(stage)) {
            return self
                .editor
                .message("The review's thread has ended: g starts it again");
        }
        decisions.staging.push_back(Staging {
            serial,
            path: path.into(),
            hunks,
            reverse,
            undo,
        });
        let verb = if reverse { "Unstaging" } else { "Staging" };
        self.editor.message(format!("{verb}\u{2026}"));
    }

    /// What the review's thread did with stage `serial`.
    pub(super) fn review_staged(&mut self, index: usize, serial: u64, staged: Staged) {
        let decisions = &mut self.reviews.open[index].decisions;
        let Some(at) = decisions.staging.iter().position(|it| it.serial == serial) else {
            return;
        };
        let Some(staging) = decisions.staging.remove(at) else {
            return;
        };
        let patch = match staged {
            Ok(patch) => patch,
            Err(why) => return self.editor.message(why),
        };
        let Staging {
            path,
            hunks,
            reverse,
            undo,
            ..
        } = staging;
        let id = self.reviews.open[index].id;
        // Staged where the scope keeps showing it, the hunk is marked; else it leaves (Q-G).
        let stays = !reverse && !matches!(self.reviews.open[index].scope, Scope::Unstaged);
        if let Some(model) = self.editor.review_mut(id) {
            for hunk in &hunks {
                if let Some(at) = hunk_now(model, &path, hunk) {
                    let decision = (stays && !undo).then_some(Decision::Accepted);
                    model.decide(&path, at, decision);
                }
            }
        }
        let count = hunks.len();
        if !undo {
            self.keep_done(
                index,
                Done::Staged {
                    path: path.clone(),
                    patch,
                    reverse,
                    hunks,
                },
            );
        }
        let noun = if count == 1 { "hunk" } else { "hunks" };
        let verb = if reverse { "Unstaged" } else { "Staged" };
        let name = String::from_utf8_lossy(&path).into_owned();
        self.editor
            .message(format!("{verb} {count} {noun} of {name}"));
        self.tell_cards(index, &path);
        self.refresh_review(id);
    }

    /// Every stage waiting fails as the thread could not answer.
    pub(super) fn stages_failed(&mut self, index: usize) {
        self.reviews.open[index].decisions.staging.clear();
    }

    /// Reverts `hunks` of `path` through the file's buffer, opened unshown if it is not open.
    fn revert_hunks(&mut self, index: usize, path: &[u8], hunks: &[usize]) {
        let hunks = match self.decided_hunks(index, path, hunks) {
            Ok((_, hunks)) => hunks,
            Err(why) => return self.editor.message(why),
        };
        self.start_revert(index, path, hunks, false);
    }

    fn start_revert(&mut self, index: usize, path: &[u8], hunks: Vec<Hunk>, undo: bool) {
        let review = self.reviews.open[index].id;
        let Some(file) = file_at(self.reviews.open[index].root.path(), path) else {
            return self.editor.message("The file's name cannot be opened here");
        };
        let open = (self.editor.buffers())
            .find(|(_, buffer)| buffer.path() == Some(file.as_path()))
            .map(|(id, _)| id);
        let buffer = match open {
            Some(buffer) => buffer,
            None => match self.visit(file, oso_core::load::UNASKED) {
                Some(buffer) => buffer,
                None => return self.editor.message("The file could not be opened"),
            },
        };
        self.reviews.open[index]
            .decisions
            .reverting
            .push(Reverting {
                review,
                path: path.into(),
                buffer,
                hunks,
                undo,
                saving: false,
            });
        self.go_on_reverting();
    }

    /// Makes the edit of each revert whose buffer has loaded, and starts its save.
    pub(super) fn go_on_reverting(&mut self) {
        for index in 0..self.reviews.open.len() {
            let mut waiting = std::mem::take(&mut self.reviews.open[index].decisions.reverting);
            waiting.retain_mut(|reverting| reverting.saving || !self.revert_now(reverting));
            self.reviews.open[index].decisions.reverting.extend(waiting);
        }
    }

    /// Makes `reverting`'s edit once its buffer has loaded, and starts its save. Returns whether
    /// it is done waiting for the load: edited and saving, or given up, said why.
    fn revert_now(&mut self, reverting: &mut Reverting) -> bool {
        let Some(buffer) = self.editor.buffer(reverting.buffer) else {
            self.editor
                .message("The file's buffer has gone: nothing reverted");
            return true;
        };
        if buffer.loading().is_some() {
            return false;
        }
        let edits = match revert_edits(buffer.text(), &reverting.hunks, reverting.undo) {
            Ok(edits) => edits,
            Err(why) => {
                self.editor.message(why);
                return true;
            }
        };
        let replacements: Vec<Replacement<'_>> = (edits.iter())
            .map(|(range, text)| Replacement {
                range: range.clone(),
                text,
            })
            .collect();
        if let Err(e) = self.editor.apply_edits(reverting.buffer, &replacements) {
            self.editor.message(format!("Not reverted: {e}"));
            return true;
        }
        if let Err(e) = self.editor.start_save_of(reverting.buffer) {
            self.editor.message(format!("Reverted, not saved: {e}"));
            return true;
        }
        reverting.saving = true;
        false
    }

    /// The save of `buffer` ended, written or not: a revert waiting for it is done.
    pub(in crate::app) fn review_save_ended(&mut self, buffer: BufferId, written: bool) {
        for index in 0..self.reviews.open.len() {
            let decisions = &mut self.reviews.open[index].decisions;
            let Some(at) = (decisions.reverting.iter())
                .position(|reverting| reverting.saving && reverting.buffer == buffer)
            else {
                continue;
            };
            let reverting = decisions.reverting.remove(at);
            if !written {
                self.editor
                    .message("Reverted in the buffer, not saved: see the message above");
                continue;
            }
            let Reverting {
                review,
                path,
                hunks,
                undo,
                ..
            } = reverting;
            let count = hunks.len();
            let noun = if count == 1 { "hunk" } else { "hunks" };
            let name = String::from_utf8_lossy(&path).into_owned();
            let verb = if undo { "Put back" } else { "Reverted" };
            self.editor
                .message(format!("{verb} {count} {noun} of {name}"));
            if !undo {
                self.keep_done(index, Done::Reverted { path, hunks });
            }
            self.refresh_review(review);
        }
    }

    /// Marks hunks `hunks` of `path` viewed, or clears the mark when each has it.
    pub(super) fn view_hunks(&mut self, review: ReviewId, path: &[u8], hunks: &[usize]) {
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        let Some(model) = self.editor.review_mut(review) else {
            return;
        };
        let viewed = !hunks.iter().all(|at| model.viewed(path, *at));
        let shape = model.file(path).cloned();
        let marked: Vec<Hunk> = (hunks.iter())
            .filter(|at| model.set_viewed(path, **at, viewed))
            .filter_map(|at| shape.as_ref()?.hunks().get(*at).cloned())
            .collect();
        if marked.is_empty() {
            return self.editor.message("Not on a hunk");
        }
        self.keep_done(
            index,
            Done::Viewed {
                path: path.into(),
                hunks: marked,
                viewed,
            },
        );
        self.tell_cards(index, path);
    }

    /// Takes back the review's last decision.
    pub(super) fn undo_decision(&mut self, review: ReviewId) {
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        let Some(done) = self.reviews.open[index].decisions.done.pop_back() else {
            return self.editor.message("No decision to undo");
        };
        match done {
            Done::Staged {
                path,
                patch,
                reverse,
                hunks,
            } => {
                let apply = Apply::Patch(patch);
                self.ask_stage(index, &path, apply, hunks, !reverse, true);
            }
            Done::Reverted { path, hunks } => self.start_revert(index, &path, hunks, true),
            Done::Viewed {
                path,
                hunks,
                viewed,
            } => {
                if let Some(model) = self.editor.review_mut(review) {
                    for hunk in &hunks {
                        if let Some(at) = hunk_now(model, &path, hunk) {
                            model.set_viewed(&path, at, !viewed);
                        }
                    }
                }
                self.editor.message("Viewed mark undone");
                self.tell_cards(index, &path);
            }
        }
    }

    fn keep_done(&mut self, index: usize, done: Done) {
        let done_list = &mut self.reviews.open[index].decisions.done;
        if done_list.len() == MAX_DONE {
            done_list.pop_front();
        }
        done_list.push_back(done);
    }
}

#[cfg(test)]
#[path = "decide_tests.rs"]
mod tests;
