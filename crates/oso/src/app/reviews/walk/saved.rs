//! A review of a pull request's commit as its saved file names it (ADR 0167): the pull request
//! it was stepped to from, written in its header, and linked again from it once that pull
//! request's commits are listed.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or `gh`: a
//! review of the pull request is never opened for a saved file.

use super::{Spot, commit_id};
use crate::app::App;
use crate::app::reviews::state::Running;
use crate::review::Scope;

impl App {
    /// The pull request number the review `running` was stepped to from, linked or brought
    /// back to be linked, for its saved file.
    pub(in crate::app::reviews) fn pull_number(&self, running: &Running) -> Option<u64> {
        let linked = (running.pull)
            .and_then(|(pull, _)| self.reviews.index(pull))
            .and_then(|at| self.reviews.open[at].walk.as_ref())
            .map(|walk| walk.number);
        linked.or(running.pull_wanted)
    }

    /// The review at `index`, of one commit, was saved as stepped to from pull request
    /// `number`: it is linked once that pull request's commits are listed, now if they are.
    /// No review of the pull request is opened for it: that asks `gh`, which only the person's
    /// own open of the pull request or `g` does (ADR 0122, step 12b).
    pub(in crate::app::reviews) fn want_pull(&mut self, index: usize, number: Option<u64>) {
        let Some(number) = number else {
            return;
        };
        let running = &mut self.reviews.open[index];
        let (review, project) = (running.id, running.project);
        running.pull_wanted = Some(number);
        let scope = Scope::PullRequest { number };
        let Some(pull_index) = (self.reviews.open.iter())
            .position(|other| other.project == project && other.scope == scope)
        else {
            return;
        };
        let at = (self.reviews.open[pull_index].walk.as_ref())
            .zip(commit_id(&self.reviews.open[index]))
            .and_then(|(walk, id)| walk.index_of(id));
        if let Some(at) = at {
            let pull = self.reviews.open[pull_index].id;
            if let Some(walk) = self.reviews.open[pull_index].walk.as_mut() {
                walk.used(at, review);
                walk.dirty = true;
            }
            self.link(review, pull, Spot::Commit(at));
        }
    }
}
