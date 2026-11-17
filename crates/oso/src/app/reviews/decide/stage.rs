//! Accept, and unstage in the staged scope (ADR 0122, step 5): stages asked of the review's
//! thread, each answered by its ticket however it ends, and what each answer does to the marks,
//! the undo list and the review.
//!
//! Must not depend on: key presses or how a frame is drawn; the review's thread runs git.
//!
//! - Staged where the scope keeps showing the hunk, uncommitted or the session's, it is marked
//!   accepted; staged in the unstaged scope, or unstaged, it leaves, its mark taken off (Q-G).
//! - At most [`MAX_STAGING`] stages wait at once.
//! - A stage that fails, or an undo of one, says why; a failed undo keeps its decision.

use std::sync::Arc;

use oso_core::review::{Base, Decision};

use super::{Done, Marking, Picked, hunks_said, named};
use crate::app::App;
use crate::git::Id;
use crate::review::stage::{Apply, Stage, Staged};
use crate::review::{Request, Scope};

/// The most stages a review waits for at once.
const MAX_STAGING: usize = 16;

/// A stage asked of the review's thread.
pub(super) struct Staging {
    ticket: u64,
    picked: Picked,
    reverse: bool,
    /// The decision this stage undoes, kept back if it fails.
    undoing: Option<Done>,
}

impl App {
    /// Asks the review's thread to stage `picked`, or to unstage it with `reverse`.
    pub(super) fn stage_picked(&mut self, index: usize, picked: Picked, reverse: bool) {
        let Base::Blob(old) = picked.shape.base() else {
            return self
                .editor
                .message("A new file: M-x delete-file removes it");
        };
        let Some(old) = Id::parse(old) else {
            return self.editor.message("Not shown as text: nothing to decide");
        };
        let apply = Apply::Hunks {
            old,
            hunks: picked.hunks().cloned().collect(),
        };
        self.ask_stage(index, picked, apply, reverse, None);
    }

    /// Asks for the decision `done`'s patch the other way, keeping it back if that fails.
    pub(super) fn unstage_done(
        &mut self,
        index: usize,
        picked: Picked,
        patch: Arc<[u8]>,
        reverse: bool,
    ) {
        let apply = Apply::Patch(Arc::clone(&patch));
        let done = Done::Staged {
            picked: picked.clone(),
            patch,
            reverse,
        };
        self.ask_stage(index, picked, apply, !reverse, Some(done));
    }

    fn ask_stage(
        &mut self,
        index: usize,
        picked: Picked,
        apply: Apply,
        reverse: bool,
        undoing: Option<Done>,
    ) {
        let running = &mut self.reviews.open[index];
        let decisions = &mut running.decisions;
        let asked = match &running.reviewer {
            None => Err("The review is not reading: g starts it again"),
            Some(_) if decisions.staging.len() >= MAX_STAGING => {
                Err("Too many decisions wait: try in a moment")
            }
            Some(reviewer) => {
                let ticket = decisions.next_ticket;
                decisions.next_ticket += 1;
                let stage = Stage {
                    ticket,
                    path: picked.path.clone(),
                    apply,
                    reverse,
                };
                if reviewer.ask(Request::Stage(stage)) {
                    Ok(ticket)
                } else {
                    Err("The review's thread has ended: g starts it again")
                }
            }
        };
        let ticket = match asked {
            Ok(ticket) => ticket,
            Err(why) => {
                if let Some(done) = undoing {
                    self.keep_undone(index, done);
                }
                return self.editor.message(why);
            }
        };
        self.reviews.open[index].decisions.staging.push(Staging {
            ticket,
            picked,
            reverse,
            undoing,
        });
        let verb = if reverse { "Unstaging" } else { "Staging" };
        self.editor.message(format!("{verb}\u{2026}"));
    }

    /// What the review's thread did with the stage `ticket`.
    pub(in crate::app::reviews) fn review_staged(
        &mut self,
        index: usize,
        ticket: u64,
        staged: Staged,
    ) {
        let staging = &mut self.reviews.open[index].decisions.staging;
        let Some(at) = staging.iter().position(|it| it.ticket == ticket) else {
            return;
        };
        let Staging {
            picked,
            reverse,
            undoing,
            ..
        } = staging.remove(at);
        let patch = match staged {
            Ok(patch) => patch,
            Err(why) => {
                if let Some(done) = undoing {
                    self.keep_undone(index, done);
                }
                let why = crate::commands::review::one_line(&why, MAX_MESSAGE);
                return self.editor.message(why);
            }
        };
        // In the work tree's scopes that compare with HEAD or the baseline, the index changes
        // nothing shown: the hunk stays, marked accepted or not, its viewed mark kept. In the
        // staged and unstaged scopes it leaves, its whole mark with it (Q-G).
        let scope = &self.reviews.open[index].scope;
        let stays = matches!(scope, Scope::Uncommitted | Scope::Session { .. });
        let mark = if !stays {
            Marking::Leaves
        } else if reverse {
            Marking::Decided(None)
        } else {
            Marking::Decided(Some(Decision::Accepted))
        };
        self.mark_picked(index, &picked, mark);
        let verb = if reverse { "Unstaged" } else { "Staged" };
        let said = format!(
            "{verb} {} of {}",
            hunks_said(picked.hunks.len()),
            named(&picked.path)
        );
        if undoing.is_none() {
            self.keep_done(
                index,
                Done::Staged {
                    picked,
                    patch,
                    reverse,
                },
            );
        }
        self.editor.message(said);
        let id = self.reviews.open[index].id;
        self.refresh_review(id);
    }

    /// The review's thread starts over: what it was asked to stage is answered by no one, so
    /// it is forgotten, said once, its undo kept.
    pub(in crate::app::reviews) fn stages_lost(&mut self, index: usize) {
        let lost = std::mem::take(&mut self.reviews.open[index].decisions.staging);
        if lost.is_empty() {
            return;
        }
        for staging in lost {
            if let Some(done) = staging.undoing {
                self.keep_undone(index, done);
            }
        }
        self.editor
            .message("The review started over: what was staging may not have gone in; g shows it");
    }
}

/// The most characters of what git said a message keeps.
const MAX_MESSAGE: usize = 200;
