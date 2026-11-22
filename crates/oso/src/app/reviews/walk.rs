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

use oso_core::editor::{CommitEntry, Commits, ReviewId, Step};
use oso_core::review::{Author, Review};

use super::state::Running;
use crate::app::App;
use crate::commands::review::one_line;
use crate::review::Scope;
use crate::review::commits::Listed;

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
    /// The commit shown last, or `None` for the whole pull request.
    shown: Option<usize>,
    /// The review the selected window showed when last looked at, followed only once it changed.
    selected: Option<ReviewId>,
    /// Whether the editor's commits are not these yet.
    dirty: bool,
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
            });
        }
        Walk {
            number,
            ids,
            entries: Arc::from(entries),
            more: listed.more,
            open: Vec::new(),
            counted: HashMap::new(),
            shown: None,
            selected: None,
            dirty: true,
        }
    }

    /// The commits as the editor shows them.
    fn commits(&self) -> Commits {
        Commits {
            number: self.number,
            entries: Arc::clone(&self.entries),
            more: self.more,
            shown: self.shown,
            open: self.open.clone(),
        }
    }

    /// The index of the commit `id`, a full id.
    pub(super) fn index_of(&self, id: &str) -> Option<usize> {
        self.ids.iter().position(|listed| listed == id)
    }

    /// Where a step `to` from `here`, a commit's index or `None` for the whole, goes: a commit,
    /// the whole as `Ok(None)`, or why it stays.
    fn target(&self, here: Option<usize>, to: Step) -> Result<Option<usize>, String> {
        let number = self.number;
        let on = |index: &usize| !self.entries[*index].merge && !self.entries[*index].empty;
        match to {
            Step::Whole => Ok(None),
            Step::Commit(index) if index >= self.entries.len() => {
                Err(format!("#{number} has no such commit"))
            }
            // A merge shows against its first parent, as `c` shows one.
            Step::Commit(index) => Ok(Some(index)),
            Step::Next => {
                let from = here.map_or(0, |at| at + 1);
                match (from..self.entries.len()).find(on) {
                    Some(index) => Ok(Some(index)),
                    None if here.is_none() => {
                        Err(format!("#{number} has no commit to review alone"))
                    }
                    None => Err(format!(
                        "{} is the last commit of #{number}",
                        here.map_or("", |at| &self.entries[at].short)
                    )),
                }
            }
            Step::Previous => match here {
                None => (0..self.entries.len())
                    .rev()
                    .find(on)
                    .map(Some)
                    .ok_or_else(|| format!("#{number} has no commit to review alone")),
                Some(at) => Ok((0..at).rev().find(on)),
            },
        }
    }

    /// Marks the commit review `review` used now, the last of those open.
    fn used(&mut self, index: usize, review: ReviewId) {
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
            walk.shown = before.shown.and_then(|at| walk.index_of(&before.ids[at]));
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
            self.link(review, pull, at);
        }
        walk.dirty = true;
        self.freer.free(before);
        self.reviews.open[index].walk = Some(walk);
        self.sync_review_walks();
    }

    /// Links the commit review `review` to the pull request review `pull`, as its commit `at`;
    /// the editor links them with the walk's next commits, which name it open.
    fn link(&mut self, review: ReviewId, pull: ReviewId, at: usize) {
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
        }
        if let Some(walk) = self.reviews.open[index].walk.take() {
            for (_, review) in &walk.open {
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
            (Some(walk), _) if stay => (review, walk.shown),
            (Some(_), _) => (review, None),
            (None, Some((pull, at))) => (pull, Some(at)),
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
        let target = match walk.target(here, to) {
            Ok(target) => target,
            Err(why) => return self.editor.message(why),
        };
        let selected = self.editor.window().id();
        // The review stepped from: from the list, the one it shows.
        let from = match (stay, here) {
            (true, Some(at)) => walk
                .open
                .iter()
                .find(|(open, _)| *open == at)
                .map(|(_, id)| *id),
            _ => Some(review),
        };
        let files = from.is_some_and(|from| self.editor.review_files(from).is_some());
        let shown = match target {
            None => Some(pull),
            Some(at) => self.open_commit(pull_index, at),
        };
        let Some(shown) = shown else {
            return;
        };
        let selected_review = self.editor.review_of(self.editor.tiled().0.buffer());
        if let Some(walk) = self.reviews.open[pull_index].walk.as_mut() {
            walk.shown = target;
            walk.selected = selected_review;
            walk.dirty = true;
            if let Some(at) = target {
                walk.used(at, shown);
            }
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
        self.link(review, pull, at);
        if let Some(walk) = self.reviews.open[pull_index].walk.as_mut() {
            walk.used(at, review);
        }
        Some(review)
    }

    /// Shows the review `id` where a step shows it: in the review layout, as a person's open
    /// does (ADR 0160), else in the selected window.
    fn show_stepped(&mut self, id: ReviewId) {
        if !self.show_review_laid(id)
            && let Some(buffer) = self.editor.review_buffer(id)
        {
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

    /// Says where a step went: the whole pull request, or which of its commits.
    fn say_step(&mut self, pull_index: usize, target: Option<usize>) {
        let Some(walk) = self.reviews.open[pull_index].walk.as_ref() else {
            return;
        };
        let total = walk.entries.len();
        let said = match target {
            None => format!(
                "Reviewing #{} whole, its {total} {}: M-n steps to its first",
                walk.number,
                if total == 1 { "commit" } else { "commits" }
            ),
            Some(at) => {
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
                Some(id) if id == pull => Some(None),
                Some(id) => walk
                    .open
                    .iter()
                    .find(|(_, open)| *open == id)
                    .map(|(at, _)| Some(*at)),
                None => None,
            };
            if let Some(here) = here
                && walk.selected != selected
                && here != walk.shown
            {
                walk.shown = here;
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

    /// The pull request number the review `running` was stepped to from, linked or brought
    /// back to be linked, for its saved file.
    pub(super) fn pull_number(&self, running: &Running) -> Option<u64> {
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
    pub(super) fn want_pull(&mut self, index: usize, number: Option<u64>) {
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
            self.link(review, pull, at);
        }
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
fn commit_id(running: &Running) -> Option<&str> {
    match &running.scope {
        Scope::Commit { revision } => Some(revision),
        _ => None,
    }
}

#[cfg(test)]
#[path = "walk_tests.rs"]
mod tests;
