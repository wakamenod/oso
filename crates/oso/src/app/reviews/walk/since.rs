//! What changed in a pull request since the person's last review of it (ADR 0167, step 2):
//! the checkpoint, its head and where it left its base when the person's comments were last
//! pasted or `M-x review-checkpoint` recorded it; the commits new since, found on the review's
//! own thread; and the reviews of what changed since and of a range of commits.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git: the review's
//! thread finds what is new, and the reviews open as any does.
//!
//! - The checkpoint is the pull request's review's, saved with it. A listing whose head is the
//!   checkpoint's has nothing new; one past it asks what is new, the latest ask alone answered.
//! - A commit is new when the head then is an ancestor of the head now and the commit comes
//!   after it; after a rebase, when its patch-id is not one the commits then had, or when it
//!   changes something and no patch of it was read.
//! - A paste records the head the message was made from, not the one listed by the paste.

use std::sync::Arc;

use oso_core::editor::{ReviewId, Since, Step};

use super::Walk;
use crate::app::App;
use crate::git::Id;
use crate::review::commits::{Checkpoint, SinceAsk, SinceFound};
use crate::review::{Request, Scope};

/// Where a pull request's walk is, or goes, and what a review linked to it is of: the whole, what
/// changed since, a commit by its index, or a range of commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::reviews) enum Spot {
    Whole,
    Since,
    Commit(usize),
    /// Its commits from the first index to the second.
    Range(usize, usize),
}

impl Walk {
    /// Where the walk showed last.
    pub(super) fn spot(&self) -> Spot {
        self.shown
    }

    /// Records `spot` as shown last; a range keeps the spot it was asked from.
    pub(super) fn show(&mut self, spot: Spot) {
        if !matches!(spot, Spot::Range(..)) {
            self.shown = spot;
        }
    }

    /// Records `spot` as shown last by `review`, the one used last there.
    pub(super) fn put(&mut self, spot: Spot, review: ReviewId) {
        self.show(spot);
        match spot {
            Spot::Commit(at) => self.used(at, review),
            Spot::Since => self.since_review = Some(review),
            Spot::Whole | Spot::Range(..) => {}
        }
    }

    /// The review open at `spot`, the whole's being `pull`.
    pub(super) fn review_at(&self, spot: Spot, pull: ReviewId) -> Option<ReviewId> {
        match spot {
            Spot::Whole => Some(pull),
            Spot::Since => self.since_review,
            Spot::Commit(at) => (self.open.iter())
                .find(|(open, _)| *open == at)
                .map(|(_, id)| *id),
            Spot::Range(..) => None,
        }
    }

    /// Whether a step stops on the commit at `index`: no merge and changing something.
    fn on(&self, index: usize) -> bool {
        !self.entries[index].merge && !self.entries[index].empty
    }

    /// Where a step `to` from `here` goes, or why it stays.
    pub(super) fn spot_to(&self, here: Spot, to: Step) -> Result<Spot, String> {
        let (number, total) = (self.number, self.entries.len());
        let none = || format!("#{number} has no commit to review alone");
        let first_after = |at: usize| (at..total).find(|index| self.on(*index));
        let last_before = |at: usize| (0..at).rev().find(|index| self.on(*index));
        match to {
            Step::Whole => Ok(Spot::Whole),
            Step::Since => match self.since {
                Some(since) if since.gone => Err(format!(
                    "#{number}'s head at your last review is gone from this clone: review it \
                     whole"
                )),
                Some(_) => Ok(Spot::Since),
                None => Err(format!(
                    "Nothing in #{number} is new since your last review, or none was recorded: \
                     a paste with C-c C-c, or M-x review-checkpoint, records one"
                )),
            },
            Step::Commit(index) if index >= total => Err(format!("#{number} has no such commit")),
            // A merge shows against its first parent, as `c` shows one.
            Step::Commit(index) => Ok(Spot::Commit(index)),
            Step::Range(a, b) if a.max(b) >= total => Err(format!("#{number} has no such commit")),
            Step::Range(a, b) if a == b => Ok(Spot::Commit(a)),
            Step::Range(a, b) => Ok(Spot::Range(a.min(b), a.max(b))),
            Step::Next => match here {
                Spot::Whole => first_after(0).map(Spot::Commit).ok_or_else(none),
                // The first new one, else the first.
                Spot::Since => (0..total)
                    .find(|index| self.entries[*index].new && self.on(*index))
                    .or_else(|| first_after(0))
                    .map(Spot::Commit)
                    .ok_or_else(none),
                Spot::Commit(at) | Spot::Range(_, at) => {
                    first_after(at + 1).map(Spot::Commit).ok_or_else(|| {
                        format!("{} is the last commit of #{number}", self.entries[at].short)
                    })
                }
            },
            Step::Previous => match here {
                Spot::Whole => last_before(total).map(Spot::Commit).ok_or_else(none),
                Spot::Since => Ok(Spot::Whole),
                Spot::Commit(at) | Spot::Range(at, _) => {
                    Ok(last_before(at).map_or(Spot::Whole, Spot::Commit))
                }
            },
        }
    }

    /// Keeps what `before`, the walk this listing replaces, found new since, when it was found
    /// for the same base and head: its row, and the commits it marked by their ids.
    pub(super) fn carry_since(&mut self, before: &Walk) {
        if (self.base.as_ref(), self.tip.as_ref()) != (before.base.as_ref(), before.tip.as_ref())
            || before.answered.is_none()
        {
            return;
        }
        self.since = before.since;
        self.answered.clone_from(&before.answered);
        let new: std::collections::HashSet<&String> = (before.ids.iter())
            .zip(before.entries.iter())
            .filter(|(_, entry)| entry.new)
            .map(|(id, _)| id)
            .collect();
        if !new.is_empty() {
            for (entry, id) in Arc::make_mut(&mut self.entries).iter_mut().zip(&self.ids) {
                entry.new = new.contains(id);
            }
        }
    }

    /// Marks no commit new any more, as nothing is new since a checkpoint at the head.
    pub(super) fn none_new(&mut self) {
        self.answered = None;
        self.since = None;
        if self.entries.iter().any(|entry| entry.new) {
            for entry in Arc::make_mut(&mut self.entries) {
                entry.new = false;
            }
        }
        self.dirty = true;
    }
}

impl App {
    /// After the walk at `index` was listed: nothing is new when the head is its checkpoint's,
    /// else the review's thread is asked what is.
    pub(super) fn ask_since(&mut self, index: usize) {
        let running = &mut self.reviews.open[index];
        let (Some(checkpoint), Some(walk)) = (&running.checkpoint, running.walk.as_mut()) else {
            return;
        };
        let (Some(base), Some(tip)) = (walk.base.clone(), walk.tip.clone()) else {
            return;
        };
        if checkpoint.head == tip {
            return walk.none_new();
        }
        let ask = SinceAsk {
            from: checkpoint.clone(),
            base,
            tip,
        };
        // Answered already, as a listing carried from the one before keeps it.
        if walk.answered.as_ref() == Some(&ask) {
            return;
        }
        self.ask_review(index, Request::Since(ask));
    }

    /// The review at `index`'s thread found what changed since its checkpoint: kept and marked,
    /// unless the head or the checkpoint moved since it was asked.
    pub(in crate::app::reviews) fn review_since_found(&mut self, index: usize, found: SinceFound) {
        let running = &mut self.reviews.open[index];
        let current = running
            .checkpoint
            .as_ref()
            .map(|checkpoint| &checkpoint.head);
        let Some(walk) = running.walk.as_mut() else {
            return;
        };
        if walk.tip.as_ref() != Some(&found.ask.tip) || current != Some(&found.ask.from.head) {
            return;
        }
        // Stopped by a newer request: asked again once that one has read.
        if found.stopped {
            walk.again = true;
            return;
        }
        // After a rebase, a commit that changes something and has no patch read is new: what is
        // not known to have been reviewed counts as not.
        let unread = |id: &String, entry: &oso_core::editor::CommitEntry| {
            found.rebased && !entry.merge && !entry.empty && !found.patched.contains(id)
        };
        let new: Vec<bool> = (walk.ids.iter().zip(walk.entries.iter()))
            .map(|(id, entry)| found.new.contains(id) || unread(id, entry))
            .collect();
        if walk
            .entries
            .iter()
            .map(|entry| entry.new)
            .ne(new.iter().copied())
        {
            for (entry, new) in Arc::make_mut(&mut walk.entries).iter_mut().zip(&new) {
                entry.new = *new;
            }
            walk.dirty = true;
        }
        let since = Some(Since {
            new: new.iter().filter(|new| **new).count(),
            rebased: found.rebased,
            gone: found.gone,
            shown: false,
            review: None,
        });
        walk.dirty |= walk.since != since;
        walk.since = since;
        walk.answered = Some(found.ask);
        self.sync_review_walks();
        self.request_redraw();
    }

    /// Whether the review `review` reviews what changed since up to `tip`.
    pub(super) fn since_reviews_to(&self, review: ReviewId, tip: Option<&Id>) -> bool {
        let scope = (self.reviews.index(review)).map(|at| &self.reviews.open[at].scope);
        matches!(scope, Some(Scope::Since { to, .. }) if Some(to) == tip)
    }

    /// After the review at `index` read its files: what changed since is asked again, if a newer
    /// request stopped the last ask.
    pub(in crate::app::reviews) fn ask_since_again(&mut self, index: usize) {
        let again = (self.reviews.open[index].walk.as_mut())
            .is_some_and(|walk| std::mem::take(&mut walk.again));
        if again {
            self.ask_since(index);
        }
    }

    /// The review of what changed in the walk at `pull_index` since its checkpoint, opened if
    /// it is not, and linked.
    pub(super) fn open_since(&mut self, pull_index: usize) -> Option<ReviewId> {
        let running = &self.reviews.open[pull_index];
        let (pull, project) = (running.id, running.project);
        let walk = running.walk.as_ref()?;
        if let Some(review) = walk.since_review {
            return Some(review);
        }
        let scope = Scope::Since {
            number: walk.number,
            from: running.checkpoint.as_ref()?.head.clone(),
            to: walk.tip.clone()?,
        };
        let review = self.open_scope(project, scope, super::super::Show::Hidden)?;
        self.link(review, pull, Spot::Since);
        Some(review)
    }

    /// A review of the commits `first` to `last` of the walk at `pull_index`, as `r` reviews a
    /// range: of no one commit, so not linked.
    pub(super) fn open_range(
        &mut self,
        pull_index: usize,
        first: usize,
        last: usize,
    ) -> Option<ReviewId> {
        let running = &self.reviews.open[pull_index];
        let project = running.project;
        let walk = running.walk.as_ref()?;
        let scope = Scope::Range {
            from: walk.ids.get(first)?.clone(),
            to: walk.ids.get(last)?.clone(),
        };
        self.open_scope(project, scope, super::super::Show::Hidden)
    }

    /// The checkpoint the walk at `index`, a pull request's review or one of its commits', would
    /// record now: the base and the head it lists.
    pub(in crate::app::reviews) fn walk_checkpoint(&self, index: usize) -> Option<Checkpoint> {
        let walk = self.reviews.open[self.walk_of(index)?].walk.as_ref()?;
        Some(Checkpoint {
            base: walk.base.clone()?,
            head: walk.tip.clone()?,
        })
    }

    /// Records `at`, else the head the walk at `pull_index` lists, as the person's last review of
    /// its pull request, from which what changed since is shown; the head's short id, if it did.
    /// A head listed since `at` is new again once the review's thread says so.
    pub(in crate::app::reviews) fn set_checkpoint(
        &mut self,
        pull_index: usize,
        at: Option<Checkpoint>,
    ) -> Option<String> {
        let checkpoint = match at {
            Some(at) => at,
            None => self.walk_checkpoint(pull_index)?,
        };
        let short = checkpoint.head.as_str().chars().take(7).collect();
        let running = &mut self.reviews.open[pull_index];
        let walk = running.walk.as_mut()?;
        walk.none_new();
        if let Some(review) = walk.since_review.take() {
            if walk.shown == Spot::Since {
                walk.shown = Spot::Whole;
            }
            self.unlink(review);
        }
        let running = &mut self.reviews.open[pull_index];
        running.checkpoint = Some(checkpoint);
        running.checkpoint_moves += 1;
        self.ask_since(pull_index);
        self.sync_review_walks();
        Some(short)
    }

    /// `review-checkpoint`: records the head of the pull request `review` reviews, or reviews
    /// a commit of, as the person's last review of it.
    pub(in crate::app::reviews) fn review_checkpoint(&mut self, review: ReviewId) {
        let pull = (self.reviews.index(review)).and_then(|index| self.walk_of(index));
        let said = match pull.and_then(|pull| self.set_checkpoint(pull, None)) {
            Some(short) => format!(
                "Recorded {short} as your last review: what comes after it shows as Since last \
                 review in l's list"
            ),
            None => "This review is no pull request's whose commits are listed: nothing recorded"
                .to_owned(),
        };
        self.editor.message(said);
        self.request_redraw();
    }
}
