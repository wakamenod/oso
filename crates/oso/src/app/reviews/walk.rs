//! A pull request read a commit at a time in the app (ADR 0167): its commits as its review's
//! own thread listed them, a review of one commit opened or shown for each step, and the marks
//! its commit list shows, counted from those reviews.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git: a commit's
//! review is an ordinary review of one commit, opened as any is, on its own threads.
//!
//! - A commit's review is linked to its pull request's review while both are open, and its
//!   saved file names the pull request, so that one brought back is linked again once the
//!   pull request's commits are listed.
//! - At most [`MAX_OPEN`] commit reviews of a pull request stay open; past them the one used
//!   longest ago is closed, unless a window shows it, it holds comments of the person's not
//!   sent, or a draft is open on it, so that a send gathers every comment not sent from the
//!   reviews open.
//! - The marks are counted again only from a review whose model changed since, and the list is
//!   laid out again only when a mark or the commit shown changed. The entries are one `Arc`,
//!   copied only when a mark changed, so that a step hands the editor the same entries.

use std::collections::HashMap;
use std::sync::Arc;

use oso_core::editor::{CommitEntry, Commits, ReviewId, Since, Step};
use oso_core::review::{Author, Review};

use super::state::Running;
use crate::app::App;
use crate::commands::review::one_line;
use crate::git::Id;
use crate::review::Scope;
use crate::review::commits::Listed;
pub(super) use since::Spot;

mod saved;

mod since;

/// The most reviews of a pull request's commits kept open.
pub(super) const MAX_OPEN: usize = 8;
/// The most characters of a subject kept, for the list and a send's headings.
const MAX_SUBJECT: usize = 200;

/// What the review of a pull request keeps of its commits.
#[derive(Debug)]
pub(super) struct Walk {
    pub(super) number: u64,
    /// Each commit's full id, as its entry's index.
    pub(super) ids: Vec<String>,
    entries: Arc<[CommitEntry]>,
    more: bool,
    /// The commit reviews open, by their commit's index, the one used last at the end.
    pub(super) open: Vec<(usize, ReviewId)>,
    /// Each open commit review's model revision when its marks were last counted.
    counted: HashMap<ReviewId, u64>,
    /// What the walk showed last: the whole pull request, what changed since, or a commit.
    shown: Spot,
    /// The review the selected window showed when last looked at, followed only once it changed.
    selected: Option<ReviewId>,
    /// Whether the editor's commits are not these yet.
    dirty: bool,
    /// Where the head left the base, and the head listed: what a checkpoint records (step 2).
    pub(super) base: Option<Id>,
    pub(super) tip: Option<Id>,
    /// What changed since the last review, once found, its review while open, and whether that
    /// review is the one shown last (step 2).
    since: Option<Since>,
    since_review: Option<ReviewId>,
    /// The ask of what changed since that `since` answers.
    answered: Option<crate::review::commits::SinceAsk>,
    /// Whether to ask it again, as a newer request stopped it.
    again: bool,
}

impl Walk {
    fn of(number: u64, listed: Listed) -> Walk {
        let mut ids = Vec::with_capacity(listed.commits.len());
        let mut entries = Vec::with_capacity(listed.commits.len());
        for commit in listed.commits {
            ids.push(commit.id.as_str().to_owned());
            let subject = match commit.subject.char_indices().nth(MAX_SUBJECT) {
                Some((at, _)) => format!("{}\u{2026}", &commit.subject[..at]),
                None => commit.subject,
            };
            entries.push(CommitEntry {
                short: commit.short,
                subject,
                author: commit.author,
                date: commit.date,
                merge: commit.merge,
                empty: commit.empty,
                unpushed: commit.unpushed,
                viewed: 0,
                hunks: 0,
                unsent: 0,
                new: false,
            });
        }
        Walk {
            number,
            ids,
            entries: Arc::from(entries),
            more: listed.more,
            open: Vec::new(),
            counted: HashMap::new(),
            shown: Spot::Whole,
            selected: None,
            dirty: true,
            base: listed.base,
            tip: listed.tip,
            since: None,
            since_review: None,
            answered: None,
            again: false,
        }
    }

    /// The commits as the editor shows them.
    fn commits(&self) -> Commits {
        Commits {
            number: self.number,
            entries: Arc::clone(&self.entries),
            more: self.more,
            shown: match self.shown {
                Spot::Commit(at) => Some(at),
                Spot::Whole | Spot::Since | Spot::Range(..) => None,
            },
            open: self.open.clone(),
            since: (self.since).map(|since| Since {
                shown: self.shown == Spot::Since,
                review: self.since_review,
                ..since
            }),
        }
    }

    /// The index of the commit `id`, a full id.
    pub(super) fn index_of(&self, id: &str) -> Option<usize> {
        self.ids.iter().position(|listed| listed == id)
    }

    /// Marks the commit review `review` used now, the last of those open.
    pub(super) fn used(&mut self, index: usize, review: ReviewId) {
        self.open.retain(|(_, open)| *open != review);
        self.open.push((index, review));
    }

    /// Counts the marks of the commit at `index` again from `model`, its review's, if it changed
    /// since they were last counted.
    fn count(&mut self, index: usize, review: ReviewId, model: &Review) {
        let revision = model.revision();
        if self.counted.get(&review) == Some(&revision) {
            return;
        }
        self.counted.insert(review, revision);
        let (_, viewed, hunks) = model.progress();
        let unsent = unsent(model);
        let entry = &self.entries[index];
        if (entry.viewed, entry.hunks, entry.unsent) != (viewed, hunks, unsent) {
            // A copy of the entries only now, as the editor holds the ones before.
            let entry = &mut Arc::make_mut(&mut self.entries)[index];
            (entry.viewed, entry.hunks, entry.unsent) = (viewed, hunks, unsent);
            self.dirty = true;
        }
    }
}

/// How many comments of the person's `model` holds that were not sent.
pub(super) fn unsent(model: &Review) -> usize {
    (model.comments())
        .filter(|comment| comment.author() == Author::Person && !comment.is_sent())
        .count()
}

impl App {
    /// The review at `index`'s thread listed its pull request's commits: they are kept, the
    /// commit reviews open linked again by their ids, and those a saved file names as this
    /// pull request's linked.
    pub(super) fn review_commits_listed(&mut self, index: usize, listed: Listed) {
        let running = &self.reviews.open[index];
        let Scope::PullRequest { number } = running.scope else {
            return;
        };
        let (pull, project) = (running.id, running.project);
        let mut walk = Walk::of(number, listed);
        let before = self.reviews.open[index].walk.take();
        if let Some(before) = &before {
            walk.shown = match before.shown {
                Spot::Commit(at) => {
                    (walk.index_of(&before.ids[at])).map_or(Spot::Whole, Spot::Commit)
                }
                Spot::Since => Spot::Since,
                Spot::Whole | Spot::Range(..) => Spot::Whole,
            };
            walk.carry_since(before);
            // The review of what changed since stays while it reviews up to this head.
            match before.since_review {
                Some(review) if self.since_reviews_to(review, walk.tip.as_ref()) => {
                    walk.since_review = Some(review);
                }
                Some(review) => self.unlink(review),
                None => {}
            }
            if walk.shown == Spot::Since && walk.since_review.is_none() {
                walk.shown = Spot::Whole;
            }
            for (_, review) in &before.open {
                let entry = (self.reviews.index(*review))
                    .and_then(|at| commit_id(&self.reviews.open[at]))
                    .and_then(|id| walk.index_of(id));
                match entry {
                    Some(at) => walk.open.push((at, *review)),
                    None => self.unlink(*review),
                }
            }
        }
        // Those brought back that name this pull request (step 6's saved file).
        let wanted: Vec<(ReviewId, usize)> = (self.reviews.open.iter())
            .filter(|other| {
                other.project == project
                    && other.pull.is_none()
                    && other.pull_wanted == Some(number)
            })
            .filter_map(|other| Some((other.id, walk.index_of(commit_id(other)?)?)))
            .collect();
        for (review, at) in wanted {
            walk.open.push((at, review));
        }
        for (at, review) in walk.open.clone() {
            self.link(review, pull, Spot::Commit(at));
        }
        walk.dirty = true;
        self.freer.free(before);
        self.reviews.open[index].walk = Some(walk);
        // What changed since the last review, found on the review's thread (step 2).
        self.ask_since(index);
        self.sync_review_walks();
    }

    /// Links the review `review` to the pull request review `pull`, as its commit or what
    /// changed since, `at`; the editor links them with the walk's next commits, which name it.
    pub(super) fn link(&mut self, review: ReviewId, pull: ReviewId, at: Spot) {
        if let Some(index) = self.reviews.index(review) {
            self.reviews.open[index].pull = Some((pull, at));
            self.reviews.open[index].pull_wanted = None;
        }
    }

    /// Links the commit review `review` to no pull request any more.
    fn unlink(&mut self, review: ReviewId) {
        if let Some(index) = self.reviews.index(review) {
            self.reviews.open[index].pull = None;
        }
    }

    /// The review at `index` closes: a commit's review leaves its pull request's walk, and a
    /// pull request's unlinks its commits' reviews, which stay open as any review of a commit.
    pub(super) fn walk_review_closed(&mut self, index: usize) {
        let id = self.reviews.open[index].id;
        if let Some((pull, _)) = self.reviews.open[index].pull
            && let Some(walk) =
                (self.reviews.index(pull)).and_then(|at| self.reviews.open[at].walk.as_mut())
        {
            walk.open.retain(|(_, open)| *open != id);
            walk.counted.remove(&id);
            if walk.since_review == Some(id) {
                walk.since_review = None;
                if walk.shown == Spot::Since {
                    walk.shown = Spot::Whole;
                }
                walk.dirty = true;
            }
        }
        if let Some(walk) = self.reviews.open[index].walk.take() {
            for review in walk
                .open
                .iter()
                .map(|(_, review)| review)
                .chain(&walk.since_review)
            {
                self.unlink(*review);
                // Still its pull request's, for its saved file and a link once it opens again.
                if let Some(at) = self.reviews.index(*review) {
                    self.reviews.open[at].pull_wanted = Some(walk.number);
                }
            }
            self.freer.free(walk);
        }
    }

    /// `review-next-commit` and its kin: shows the review `to` names of the pull request
    /// `review` is, or reviews a commit of, selecting it, unless the step came from the
    /// pull request's commit list, which stays selected.
    pub(super) fn step_review(&mut self, review: ReviewId, to: Step) {
        let Some(index) = self.reviews.index(review) else {
            return;
        };
        let list = self.editor.review_commits_pane(review);
        let stay = list.is_some() && list == Some(self.editor.window().buffer());
        let running = &self.reviews.open[index];
        let (pull, here) = match (&running.walk, running.pull) {
            // From the commit list, from the review it shows last.
            (Some(walk), _) if stay => (review, walk.spot()),
            (Some(_), _) => (review, Spot::Whole),
            (None, Some((pull, at))) => (pull, at),
            (None, None) if matches!(running.scope, Scope::PullRequest { .. }) => {
                return self
                    .editor
                    .message("The pull request's commits are not listed yet: g reads them again");
            }
            (None, None) => {
                return self.editor.message(
                    "This review is no pull request's: M-n and M-p step through a pull \
                     request's commits",
                );
            }
        };
        let Some(pull_index) = self.reviews.index(pull) else {
            return;
        };
        let Some(walk) = self.reviews.open[pull_index].walk.as_ref() else {
            return;
        };
        let target = match walk.spot_to(here, to) {
            Ok(target) => target,
            Err(why) => return self.editor.message(why),
        };
        let selected = self.editor.window().id();
        // The review stepped from: from the list, the one it shows.
        let from = match (stay, here) {
            (true, spot) => walk.review_at(spot, pull),
            _ => Some(review),
        };
        let files = from.is_some_and(|from| self.editor.review_files(from).is_some());
        let shown = match target {
            Spot::Whole => Some(pull),
            Spot::Since => self.open_since(pull_index),
            Spot::Commit(at) => self.open_commit(pull_index, at),
            Spot::Range(first, last) => self.open_range(pull_index, first, last),
        };
        let Some(shown) = shown else {
            return;
        };
        let selected_review = self.editor.review_of(self.editor.tiled().0.buffer());
        if let Some(walk) = self.reviews.open[pull_index].walk.as_mut() {
            walk.put(target, shown);
            walk.selected = selected_review;
            walk.dirty = true;
        }
        // The editor links the review before it shows, so that the list stays above it.
        self.sync_review_walks();
        if Some(shown) != from {
            self.show_stepped(shown);
            // Its files pane, as the review stepped from had one.
            if files && let Some(window) = self.window_showing(shown) {
                let _ = self.editor.show_review_files(shown, window);
            }
        }
        if stay {
            self.editor.select_window(selected);
        }
        self.trim_walk(pull_index);
        self.sync_review_walks();
        self.say_step(pull_index, target);
        self.request_redraw();
    }

    /// The review of the commit at `at` of the walk of the review at `pull_index`, opened if
    /// it is not, and linked.
    fn open_commit(&mut self, pull_index: usize, at: usize) -> Option<ReviewId> {
        let running = &self.reviews.open[pull_index];
        let (pull, project) = (running.id, running.project);
        let walk = running.walk.as_ref()?;
        if let Some((_, review)) = walk.open.iter().find(|(index, _)| *index == at) {
            return Some(*review);
        }
        let revision = walk.ids[at].clone();
        let review = self.open_scope(project, Scope::Commit { revision }, super::Show::Hidden)?;
        self.link(review, pull, Spot::Commit(at));
        if let Some(walk) = self.reviews.open[pull_index].walk.as_mut() {
            walk.used(at, review);
        }
        Some(review)
    }

    /// Shows the review `id` where a step shows it: in the review layout, as a person's open
    /// does (ADR 0160), else in the selected window, in the whole area of a pair of a review's
    /// panes there.
    fn show_stepped(&mut self, id: ReviewId) {
        if !self.show_review_laid(id)
            && let Some(buffer) = self.editor.review_buffer(id)
        {
            let window = self.editor.tiled().0.id();
            self.editor.show_review_unified(window);
            self.editor.show(buffer);
        }
    }

    /// The first window showing the review `id`'s buffer or one of its panes.
    fn window_showing(&self, id: ReviewId) -> Option<oso_core::window::WindowId> {
        let buffer = self.editor.review_buffer(id)?;
        let (old, new) = self.editor.review_panes(id).unzip();
        (self.editor.windows())
            .find(|window| {
                let shown = window.buffer();
                shown == buffer || Some(shown) == old || Some(shown) == new
            })
            .map(|window| window.id())
    }

    /// Closes the commit reviews of the walk at `pull_index` past [`MAX_OPEN`], the one used
    /// longest ago first, but one a window shows, one with comments of the person's not sent
    /// and one a draft is open on.
    fn trim_walk(&mut self, pull_index: usize) {
        let Some(walk) = self.reviews.open[pull_index].walk.as_ref() else {
            return;
        };
        let mut over = walk.open.len().saturating_sub(MAX_OPEN);
        let mut closing = Vec::new();
        for (_, review) in &walk.open {
            if over == 0 {
                break;
            }
            let shown = self.window_showing(*review).is_some();
            let drafted = self.editor.draft().is_some_and(|(id, _)| id == *review);
            let unsent = self
                .editor
                .review(*review)
                .is_some_and(|model| unsent(model) > 0);
            if !(shown || drafted || unsent) {
                closing.push(*review);
                over -= 1;
            }
        }
        if let Some(walk) = self.reviews.open[pull_index].walk.as_mut() {
            walk.open.retain(|(_, open)| !closing.contains(open));
            walk.dirty |= !closing.is_empty();
        }
        for review in closing {
            self.editor.remove_review(review);
        }
    }

    /// Says where a step went: the whole pull request, what changed since its last review, a
    /// range of its commits, or which of its commits.
    fn say_step(&mut self, pull_index: usize, target: Spot) {
        let Some(walk) = self.reviews.open[pull_index].walk.as_ref() else {
            return;
        };
        let total = walk.entries.len();
        let said = match target {
            Spot::Whole => format!(
                "Reviewing #{} whole, its {total} {}: M-n steps to its first",
                walk.number,
                if total == 1 { "commit" } else { "commits" }
            ),
            Spot::Since => format!(
                "Reviewing what changed in #{} since your last review: M-n steps to its first \
                 new commit",
                walk.number
            ),
            Spot::Range(first, last) => format!(
                "Reviewing #{}'s commits {} to {} of {total}",
                walk.number,
                first + 1,
                last + 1
            ),
            Spot::Commit(at) => {
                let entry = &walk.entries[at];
                format!(
                    "Reviewing #{}, commit {}/{total} {} \u{b7} {}",
                    walk.number,
                    at + 1,
                    entry.short,
                    one_line(&entry.subject, 80)
                )
            }
        };
        self.editor.message(said);
    }

    /// After each event: counts each walk's marks again from the commit reviews whose models
    /// changed, follows the review the selected window shows once it is another, and gives the
    /// editor the commits when any of it changed. Costs a look at each open commit review's
    /// revision.
    pub(super) fn sync_review_walks(&mut self) {
        let selected = self.editor.review_of(self.editor.tiled().0.buffer());
        for index in 0..self.reviews.open.len() {
            let pull = self.reviews.open[index].id;
            let Some(mut walk) = self.reviews.open[index].walk.take() else {
                continue;
            };
            for (at, review) in walk.open.clone() {
                if let Some(model) = self.editor.review(review) {
                    walk.count(at, review, model);
                }
            }
            let here = match selected {
                Some(id) if id == pull => Some(Spot::Whole),
                Some(id) if walk.since_review == Some(id) => Some(Spot::Since),
                Some(id) => walk
                    .open
                    .iter()
                    .find(|(_, open)| *open == id)
                    .map(|(at, _)| Spot::Commit(*at)),
                None => None,
            };
            if let Some(here) = here
                && walk.selected != selected
                && here != walk.spot()
            {
                walk.show(here);
                walk.dirty = true;
            }
            walk.selected = selected;
            if walk.dirty {
                walk.dirty = false;
                self.editor.set_review_commits(pull, Some(walk.commits()));
            }
            self.reviews.open[index].walk = Some(walk);
        }
    }

    /// The reviews of the walk at `pull_index` a send gathers from, in its order: the whole
    /// pull request's, then its commits' open, oldest first, each with its group's heading.
    pub(super) fn walk_members(&self, pull_index: usize) -> Vec<(ReviewId, Option<String>)> {
        let running = &self.reviews.open[pull_index];
        let Some(walk) = running.walk.as_ref() else {
            return vec![(running.id, None)];
        };
        let mut members = vec![(running.id, Some(format!("The whole of #{}", walk.number)))];
        if let Some(review) = walk.since_review {
            let heading = format!("What changed in #{} since the last review", walk.number);
            members.push((review, Some(heading)));
        }
        let mut open = walk.open.clone();
        open.sort_unstable();
        let total = walk.entries.len();
        for (at, review) in open {
            let entry = &walk.entries[at];
            // The subject is anyone's words: quoted and cut, so it reads as no words of oso's.
            let subject: String = entry.subject.chars().take(120).collect();
            let heading = format!("Commit {}/{total} {} \"{subject}\"", at + 1, entry.short);
            members.push((review, Some(heading)));
        }
        members
    }

    /// The pull request walk the review at `index` is, or is a commit of, by the walk's index.
    pub(super) fn walk_of(&self, index: usize) -> Option<usize> {
        let running = &self.reviews.open[index];
        if running.walk.is_some() {
            return Some(index);
        }
        let (pull, _) = running.pull?;
        self.reviews
            .index(pull)
            .filter(|at| self.reviews.open[*at].walk.is_some())
    }
}

/// The full id a review of one commit, opened by a step, reviews.
pub(super) fn commit_id(running: &Running) -> Option<&str> {
    match &running.scope {
        Scope::Commit { revision } => Some(revision),
        _ => None,
    }
}

#[cfg(test)]
#[path = "walk_tests.rs"]
mod tests;
