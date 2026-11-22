//! What a review buffer's modeline says (ADR 0122): its agent comments unread, how far a reading
//! is, which commit of its pull request it is (ADR 0167) and how far the person is.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads.

use crate::buffer::BufferId;
use crate::editor::Editor;
use crate::review::Author;

use super::commits;

impl Editor {
    /// What a review buffer's modeline says after its mode, `Review`, into `out`: how many agent
    /// comments are unread, how far a reading is and how far the person is, each after a gap,
    /// as the fields after the state are. Nothing for another buffer. Costs a look at each
    /// comment.
    pub(in crate::editor) fn review_status_into(&self, buffer: BufferId, out: &mut String) {
        use std::fmt::Write as _;
        let Some(open) = self.reviews.holding(buffer) else {
            return;
        };
        let unread = open
            .review
            .comments()
            .filter(|comment| comment.author() != Author::Person && !comment.is_read())
            .count();
        if unread > 0 {
            let _ = write!(out, "  {unread} unread");
        }
        if let Some((read, listed)) = open.reading {
            let _ = write!(out, "  reading {read}/{listed}");
        }
        // Which commit of its pull request it is, or how many commits the pull request has
        // (ADR 0167).
        if let Some((pull, link)) = self.review_pull_index(open.id)
            && let Some(commits) = self.review_commits(pull)
        {
            match link {
                commits::Link::Commit(index) => {
                    let _ = write!(
                        out,
                        "  #{} {}/{}",
                        commits.number,
                        index + 1,
                        commits.entries.len()
                    );
                }
                // What changed since the last review (step 2).
                commits::Link::Since => {
                    let _ = write!(out, "  #{} since last review", commits.number);
                }
            }
        } else if let Some(commits) = open.walk.commits.as_ref() {
            let total = commits.entries.len();
            let noun = if total == 1 { "commit" } else { "commits" };
            let _ = write!(out, "  #{} all {total} {noun}", commits.number);
        }
        // How far the person is (ADR 0122, step 5).
        let (decided, viewed, hunks) = open.review.progress();
        if hunks > 0 {
            let noun = if hunks == 1 { "hunk" } else { "hunks" };
            let _ = write!(
                out,
                "  {decided} of {hunks} {noun} decided, {viewed} viewed"
            );
        }
    }
}
