//! A pull request's commits in its review (ADR 0167): the list the host read of them, which of
//! them each commit's review is, and the commit list, a strip above the review that lists them
//! a row each.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. The host reads the commits
//! on the pull request review's own thread and hands them in as plain data, with the marks it
//! counts; the strip's text is laid out here from them, a row a commit, as they are at most
//! 1,000 and each row is cut short.
//!
//! - The host lists at most 1,000 commits, the newest, and says when it left some out.
//! - A commit's review is an ordinary review of one commit, linked to the pull request's
//!   review while the commits the host hands in name it open ([`Commits::open`]): the link
//!   goes when either review closes, or the next commits no longer name it.
//! - The strip is made above a window showing a review of the pull request, or above the whole
//!   pair of its panes, at most [`STRIP_ROWS`] rows tall, and goes, its rows given back to the
//!   area below it, when it is hidden, after a command once no window shows it, or once no
//!   window shows the pull request's review or any of its commits' reviews.
//! - Its first row is the whole pull request, then a row a commit, oldest first. New commits
//!   lay out only the rows that changed, and its cursor goes to the row of the review shown
//!   only when that one changed, so that a mark counted again never moves the person's cursor.
//!   A whole new list, of another length, is laid out whole.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::buffer::{BufferId, Mode};
use crate::editor::{Editor, NoPair, ReviewId};
use crate::frame::{TooSmall, Way};
use crate::window::WindowId;

/// The most text rows the strip takes when it is made, its modeline besides.
const STRIP_ROWS: usize = 9;
/// The most characters of a commit's subject a row shows.
const MAX_SUBJECT: usize = 120;
/// The most characters of an author's name a row shows.
const MAX_AUTHOR: usize = 40;

/// A pull request's commits, as the host read them, for its review (ADR 0167).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commits {
    /// The pull request's number.
    pub number: u64,
    /// Its commits, oldest first.
    pub entries: Arc<[CommitEntry]>,
    /// Whether it has more than these, the newest: the host lists at most 1,000.
    pub more: bool,
    /// The commit whose review the pull request's walk showed last, by its index, or `None`
    /// for the whole pull request.
    pub shown: Option<usize>,
    /// The reviews of its commits open, each with its commit's index: each is linked to the
    /// pull request's review while these name it.
    pub open: Vec<(usize, ReviewId)>,
}

/// One commit of a pull request, as its row shows it. Its words are anyone's, escaped by the
/// host on one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitEntry {
    /// Its short id.
    pub short: String,
    pub subject: String,
    pub author: String,
    /// Its author's date, as `YYYY-MM-DD`.
    pub date: String,
    /// Whether it is a merge, which a step passes over.
    pub merge: bool,
    /// Whether it changes no file, which a step passes over.
    pub empty: bool,
    /// Whether it is not on the pull request's head yet, only on the branch checked out here.
    pub unpushed: bool,
    /// What the person did in its review: hunks viewed of how many, and their comments not
    /// sent. All zero for one never reviewed.
    pub viewed: usize,
    pub hunks: usize,
    pub unsent: usize,
}

/// What a pull request's review keeps of its commits and their strip.
#[derive(Debug, Default)]
pub(in crate::editor) struct Walk {
    pub(in crate::editor) commits: Option<Commits>,
    /// The strip's buffer, while it has one.
    pane: Option<BufferId>,
}

impl Editor {
    /// The commits of the pull request the review `id` reviews whole, as the host last read
    /// them, if it is one.
    pub fn review_commits(&self, id: ReviewId) -> Option<&Commits> {
        self.reviews.get(id)?.walk.commits.as_ref()
    }

    /// Gives the review `id`, a pull request's, its commits as the host read and marked them,
    /// linking the reviews of its commits they name open and unlinking the others; or takes
    /// them away with `None`. The strip, while it shows, lays out again the rows that changed.
    /// Costs a comparison of the commits, at O(1) when their entries are the same `Arc`.
    pub fn set_review_commits(&mut self, id: ReviewId, commits: Option<Commits>) {
        let Some(open) = self.reviews.get_mut(id) else {
            return;
        };
        if open.walk.commits == commits {
            return;
        }
        let before = std::mem::replace(&mut open.walk.commits, commits);
        let links: Vec<(usize, ReviewId)> = (open.walk.commits.iter())
            .flat_map(|commits| commits.open.iter().copied())
            .collect();
        for other in &mut self.reviews.open {
            if other.pull.is_some_and(|(pull, _)| pull == id) {
                other.pull = None;
            }
            if let Some((at, _)) = links.iter().find(|(_, review)| *review == other.id) {
                other.pull = Some((id, *at));
            }
        }
        if self.review_commits(id).is_none() {
            self.hide_review_commits(id);
            return;
        }
        self.lay_out_commits(id, before.as_ref());
    }

    /// The pull request review the review `id` reviews a commit of, while they are linked.
    pub fn review_pull(&self, id: ReviewId) -> Option<ReviewId> {
        let (pull, _) = self.reviews.get(id)?.pull?;
        self.reviews.get(pull).map(|_| pull)
    }

    /// The index of the commit the review `id` reviews, among its pull request's commits.
    pub(in crate::editor) fn review_pull_index(&self, id: ReviewId) -> Option<(ReviewId, usize)> {
        let (pull, index) = self.reviews.get(id)?.pull?;
        self.reviews.get(pull).map(|_| (pull, index))
    }

    /// The buffer of the pull request review `id`'s commit list, while it has one.
    pub fn review_commits_pane(&self, id: ReviewId) -> Option<BufferId> {
        self.reviews.get(id)?.walk.pane
    }

    /// Shows the pull request review `id`'s commit list in a new window above `window`, which
    /// shows that review, a commit's review linked to it or a pane of either, or above the
    /// whole pair `window` is in, as many rows tall as it has, at most [`STRIP_ROWS`], or as
    /// many as leave the area its smallest; or the window that shows it already. Returns the
    /// list's window, which it does not select. Ends isearch.
    ///
    /// # Errors
    ///
    /// [`NoPair::NoBuffer`] for a window that shows no review of the pull request, or a review
    /// with no commits, and [`NoPair::TooSmall`] where there is no room. Nothing changes.
    pub fn show_review_commits(
        &mut self,
        id: ReviewId,
        window: WindowId,
    ) -> Result<WindowId, NoPair> {
        let index = self.window_index(window).ok_or(NoPair::NoBuffer)?;
        let shown = self.reviews.holding(self.windows[index].buffer());
        let of_pull = shown
            .is_some_and(|open| open.id == id || open.pull.is_some_and(|(pull, _)| pull == id));
        let Some(count) = self.review_commits(id).map(|commits| commits.entries.len()) else {
            return Err(NoPair::NoBuffer);
        };
        if !of_pull {
            return Err(NoPair::NoBuffer);
        }
        if let Some(buffer) = self.review_commits_pane(id)
            && let Some(shown) = self.windows.iter().find(|window| window.buffer() == buffer)
        {
            return Ok(shown.id());
        }
        let new = WindowId::new(self.next_window);
        self.set_split_gutter();
        let mut splits = self.splits.clone();
        // Its rows, the whole pull request's and a row a commit, and its modeline.
        let rows = (count + 1).min(STRIP_ROWS) + 1;
        splits
            .split_aside(window, new, Way::Down, rows, &self.frame)
            .map_err(|TooSmall| NoPair::TooSmall)?;
        self.isearch_exit();
        self.splits = splits;
        self.next_window += 1;
        self.add_split_off(index, new);
        self.lay_out(self.frame.size());
        let buffer = match self.review_commits_pane(id) {
            Some(buffer) => buffer,
            None => self.make_review_commits(id),
        };
        if let Some(index) = self.window_index(new) {
            self.window_shows(index, buffer);
        }
        self.lay_out_commits(id, None);
        Ok(new)
    }

    /// Hides the pull request review `id`'s commit list: each window showing it goes, its rows
    /// given back to the area below it, or, as the frame's only window, shows the review
    /// again; and its buffer goes. Returns whether it did: not beneath a prompt (ADR 0072).
    pub fn hide_review_commits(&mut self, id: ReviewId) -> bool {
        let Some(buffer) = self.review_commits_pane(id) else {
            return false;
        };
        if self.prompt().is_some() {
            return false;
        }
        let shown: Vec<WindowId> = (self.windows.iter())
            .filter(|window| window.buffer() == buffer)
            .map(|window| window.id())
            .collect();
        let review = self.review_buffer(id);
        self.close_aside_windows(&shown, review);
        self.drop_review_commits(id);
        true
    }

    /// Makes the review `id`'s commit list's buffer, empty, named after its review buffer.
    fn make_review_commits(&mut self, id: ReviewId) -> BufferId {
        let name = (self.review_buffer(id))
            .and_then(|buffer| self.buffer(buffer))
            .map(|buffer| buffer.name().to_owned())
            .unwrap_or_default();
        let base = name.strip_suffix('*').unwrap_or(&name);
        let buffer =
            self.add_internal_buffer(format!("{base} \u{b7} commits*"), Mode::ReviewCommits);
        if let Some(shown) = self.buffer_mut(buffer) {
            shown.set_wraps(false);
        }
        if let Some(open) = self.reviews.get_mut(id) {
            open.walk.pane = Some(buffer);
        }
        buffer
    }

    /// Lets go of the review `id`'s commit list's buffer.
    fn drop_review_commits(&mut self, id: ReviewId) {
        let Some(buffer) = self
            .reviews
            .get_mut(id)
            .and_then(|open| open.walk.pane.take())
        else {
            return;
        };
        self.drop_internal_buffer(buffer);
    }

    /// Lays the review `id`'s commit list out from its commits, while it has a buffer: whole,
    /// with each window's cursor on the row of the review shown, when `before` is none, of
    /// another length or another pull request; else only the rows that changed, a cursor moved
    /// only when the review shown changed.
    fn lay_out_commits(&mut self, id: ReviewId, before: Option<&Commits>) {
        let (Some(buffer), Some(commits)) = (self.review_commits_pane(id), self.review_commits(id))
        else {
            return;
        };
        let line = row_of(commits.shown);
        let same_list = before.is_some_and(|before| {
            (before.number, before.more, before.entries.len())
                == (commits.number, commits.more, commits.entries.len())
        });
        let shown: Vec<usize> = (0..self.windows.len())
            .filter(|index| self.windows[*index].buffer() == buffer)
            .collect();
        let (Some(before), true) = (before, same_list) else {
            let text = strip_text(commits);
            self.set_internal_text(buffer, &text);
            for index in shown {
                self.put_cursor_on_line(index, line);
                self.follow_window(index);
            }
            return;
        };
        let moved = before.shown != commits.shown;
        let rows = changed_rows(before, commits);
        let lines: Vec<(usize, usize)> = shown
            .iter()
            .map(|index| (*index, self.window_line(*index)))
            .collect();
        self.replace_internal_rows(buffer, &rows);
        for (index, was) in lines {
            self.put_cursor_on_line(index, if moved { line } else { was });
            self.follow_window(index);
        }
    }

    /// Replaces the rows of the internal buffer `id` each of `rows` names, a line's whole text
    /// but its line break, with no undo record. Costs those rows.
    fn replace_internal_rows(&mut self, id: BufferId, rows: &[(usize, String)]) {
        let Some(buffer) = self.buffer(id).filter(|buffer| buffer.is_internal()) else {
            return;
        };
        let text = buffer.text();
        let lines = text.line_count();
        let edits: Vec<crate::buffer::Replacement<'_>> = (rows.iter())
            .filter(|(line, _)| *line < lines)
            .map(|(line, row)| {
                let start = text.line_start(*line);
                let end = match *line + 1 < lines {
                    true => text.line_start(*line + 1) - 1,
                    false => text.len(),
                };
                crate::buffer::Replacement {
                    range: start..end,
                    text: row,
                }
            })
            .collect();
        let Some(staged) = buffer.stage_internal(&edits) else {
            return;
        };
        let views = self.moved_views(id, &staged.display(), staged.steps(), staged.rewrap());
        // Nothing past this point can panic part way.
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.commit_internal(staged);
        }
        self.commit_views(id, views);
    }

    /// The review `id` closes: a commit's review linked to it, as a pull request's, is linked
    /// no more, and its commit list goes, even beneath a prompt.
    pub(in crate::editor) fn close_review_commits(&mut self, id: ReviewId) {
        for open in &mut self.reviews.open {
            if open.pull.is_some_and(|(pull, _)| pull == id) {
                open.pull = None;
            }
        }
        if !self.hide_review_commits(id)
            && let Some(buffer) = self.review_commits_pane(id)
        {
            let stray: Vec<WindowId> = (self.windows.iter())
                .filter(|window| window.buffer() == buffer)
                .map(|window| window.id())
                .collect();
            self.reviews.files.stray.extend(stray);
        }
        self.drop_review_commits(id);
    }

    /// After a command: drops the commit lists no window shows, and hides those whose pull
    /// request no window shows a review of.
    pub(in crate::editor) fn sync_review_commits(&mut self) {
        let panes: Vec<(ReviewId, BufferId)> = (self.reviews.open.iter())
            .filter_map(|open| Some((open.id, open.walk.pane?)))
            .collect();
        for (id, buffer) in panes {
            let shown =
                |buffer: BufferId| self.windows.iter().any(|window| window.buffer() == buffer);
            let pull_shown = (self.reviews.open.iter())
                .filter(|open| open.id == id || open.pull.is_some_and(|(pull, _)| pull == id))
                .any(|open| {
                    shown(open.buffer)
                        || (open.panes.iter()).any(|panes| panes.buffers.iter().any(|b| shown(*b)))
                });
            if !shown(buffer) {
                self.drop_review_commits(id);
            } else if !pull_shown {
                self.hide_review_commits(id);
            }
        }
    }
}

/// The line of the commit list that the review shown, a commit's by its index or the whole
/// pull request's, is on.
fn row_of(shown: Option<usize>) -> usize {
    shown.map_or(0, |index| index + 1)
}

/// The commit list's text: the whole pull request, then a row a commit, oldest first, the one
/// shown last marked, and a last row when more were left out.
pub(crate) fn strip_text(commits: &Commits) -> String {
    let total = commits.entries.len();
    let mut text = String::with_capacity(160 * (total + 2));
    text.push_str(&whole_row(commits));
    for index in 0..total {
        text.push('\n');
        text.push_str(&commit_row(commits, index));
    }
    if commits.more {
        let _ = write!(
            text,
            "\n  \u{2026} only the newest {total} commits are listed"
        );
    }
    text
}

/// The rows of the commit list that differ between `before` and `now`, which list as many
/// commits, each with its new text: the whole's and each commit's whose entry or mark of the
/// review shown changed.
fn changed_rows(before: &Commits, now: &Commits) -> Vec<(usize, String)> {
    let mut rows = Vec::new();
    if before.shown.is_none() != now.shown.is_none() {
        rows.push((0, whole_row(now)));
    }
    let same = Arc::ptr_eq(&before.entries, &now.entries);
    for index in 0..now.entries.len() {
        let mark = |commits: &Commits| commits.shown == Some(index);
        let changed = !same && before.entries[index] != now.entries[index];
        if changed || mark(before) != mark(now) {
            rows.push((index + 1, commit_row(now, index)));
        }
    }
    rows
}

/// The whole pull request's row.
fn whole_row(commits: &Commits) -> String {
    let total = commits.entries.len();
    format!(
        "{} All changes \u{b7} #{} \u{b7} {total} {}",
        mark(commits.shown.is_none()),
        commits.number,
        if total == 1 { "commit" } else { "commits" }
    )
}

/// The row of the commit at `index`.
fn commit_row(commits: &Commits, index: usize) -> String {
    let total = commits.entries.len();
    let width = total.to_string().len();
    let entry = &commits.entries[index];
    // What oso says of the commit, in its marks column, comes before anyone's words: its
    // subject, then its date, then its author.
    format!(
        "{} {:>width$}/{total} {:<6} {} {}  \u{b7} {} \u{b7} {}",
        mark(commits.shown == Some(index)),
        index + 1,
        marks(entry),
        entry.short,
        cut(&entry.subject, MAX_SUBJECT),
        entry.date,
        cut(&entry.author, MAX_AUTHOR),
    )
}

/// The mark of the review shown.
fn mark(shown: bool) -> char {
    if shown { '\u{25b6}' } else { ' ' }
}

/// A commit's marks: `merge` or `empty` for one a step passes over, `✓` every hunk viewed or
/// `◐` some, `●N` comments of the person's not sent, `↑` not pushed.
fn marks(entry: &CommitEntry) -> String {
    let mut marks = String::new();
    if entry.merge {
        marks.push_str("merge");
    } else if entry.empty {
        marks.push_str("empty");
    }
    if entry.hunks > 0 && entry.viewed >= entry.hunks {
        marks.push('\u{2713}');
    } else if entry.viewed > 0 {
        marks.push('\u{25d0}');
    }
    if entry.unsent > 0 {
        let _ = write!(marks, "\u{25cf}{}", entry.unsent.min(999));
    }
    if entry.unpushed {
        marks.push('\u{2191}');
    }
    marks
}

/// `text`, at most `max` characters, `…` past them.
fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}\u{2026}", &text[..at]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
#[path = "commits_tests.rs"]
mod tests;
