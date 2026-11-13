//! A review comment's draft and the person's removal of a comment (ADR 0122, step 3): the
//! draft is a prompt in the top float that remembers what it is for, and what it holds goes
//! to the kill ring when it is let go, as a removed thread does.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads; the host builds the
//! review's text again when asked.
//!
//! - A draft opens only with no prompt open, so there is at most one, and review texts wait
//!   while it is open, so the line it is on cannot move under it.
//! - What it is for is forgotten when its prompt closes, however it closes.

use std::collections::HashMap;

use super::{Editor, ReviewId};
use crate::buffer::Replacement;
use crate::kill::{Direction, Piece};
use crate::minibuffer::{self, CUT, Kind, PromptError, PromptId};
use crate::review::{Anchor, Author, Comment, CommentError, CommentId, Review, Side};
use crate::rope::Rope;

/// The most characters of a path a draft's label shows.
const LABEL_PATH_CHARS: usize = 80;

/// What a draft is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftFor {
    /// A new comment on a line or a hunk of the file at `path`.
    Comment { path: Box<[u8]>, anchor: Anchor },
    /// A reply to the comment `to`.
    Reply { to: CommentId },
    /// New text for the person's own comment.
    Edit { comment: CommentId },
}

/// The open draft: its prompt, its review and what it is for.
#[derive(Debug)]
pub(in crate::editor) struct Draft {
    prompt: PromptId,
    review: ReviewId,
    draft: DraftFor,
}

impl Editor {
    /// Opens a draft for `draft` in the review `review`, holding `text`, as a prompt in the top
    /// float whose label says what it is for. Its text wraps, and its answer grows with it up to
    /// 10 rows. It enters no history.
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] while any prompt is open, as a draft never opens in another, and
    /// [`PromptError::Invalid`] for a review that is not open. Nothing changes.
    pub fn open_draft(
        &mut self,
        review: ReviewId,
        draft: DraftFor,
        text: &str,
    ) -> Result<PromptId, PromptError> {
        if !self.prompts.is_empty() {
            return Err(PromptError::TooDeep);
        }
        if self.reviews.get(review).is_none() {
            return Err(PromptError::Invalid);
        }
        let label = draft_label(&draft);
        let prompt = self.open_prompt(label, Kind::Draft)?;
        let buffer = self.current().0.buffer();
        if let Some(shown) = self.buffer_mut(buffer) {
            shown.set_wraps(true);
        }
        if !text.is_empty() {
            // The cursor ends after it, as after any insertion in the selected window.
            let _ = self.apply_edits(buffer, &[Replacement { range: 0..0, text }]);
        }
        self.reviews.draft = Some(Draft {
            prompt,
            review,
            draft,
        });
        self.relayout_prompt();
        Ok(prompt)
    }

    /// The innermost prompt's draft: its review and what it is for. `None` when the innermost
    /// prompt is not a draft.
    pub fn draft(&self) -> Option<(ReviewId, &DraftFor)> {
        let draft = self.reviews.draft.as_ref()?;
        (self.prompt()?.id() == draft.prompt).then_some((draft.review, &draft.draft))
    }

    /// Whether the window at `index` is the open draft's, which wraps as a tiled window does.
    pub(in crate::editor) fn is_draft_window(&self, index: usize) -> bool {
        self.draft().is_some() && index + 1 == self.windows.len()
    }

    /// The rows the innermost prompt's answer takes as a draft, `columns` wide; `None` when it
    /// is not a draft.
    pub(in crate::editor) fn draft_answer_rows(&self, columns: usize) -> Option<usize> {
        self.draft()?;
        let buffer = self.buffer(self.prompt()?.buffer)?;
        Some(minibuffer::draft_rows(buffer.text(), columns))
    }

    /// Lays the draft out again when its text changed, as its answer may take more rows.
    pub(in crate::editor) fn draft_edited(&mut self, buffer: crate::buffer::BufferId) {
        if self.draft().is_some() && self.prompt().is_some_and(|prompt| prompt.buffer == buffer) {
            self.relayout_prompt();
        }
    }

    /// Forgets the draft of the prompt `id`, which closed.
    pub(in crate::editor) fn forget_draft(&mut self, id: PromptId) {
        if self
            .reviews
            .draft
            .as_ref()
            .is_some_and(|draft| draft.prompt == id)
        {
            self.reviews.draft = None;
        }
    }

    /// Copies the innermost draft's text, unless empty, into the kill ring before `C-g` closes
    /// it; `keyboard-quit` says so once it closed. Returns whether it was a draft.
    pub(in crate::editor) fn keep_quit_draft(&mut self) -> bool {
        if self.draft().is_none() {
            return false;
        }
        let Some(buffer) = self.prompt().map(|prompt| prompt.buffer) else {
            return false;
        };
        let len = self.buffer(buffer).map_or(0, |shown| shown.text().len());
        if len > 0 {
            let _ = self.kill_in(buffer, std::iter::once(0..len), Direction::Forward, true);
        }
        true
    }

    /// Removes the person's own comment `comment` from the review `review`, with every reply
    /// under it, and puts the thread in the kill ring, and so on the clipboard as a kill goes:
    /// each comment as its label line, `You #7` or `Agent #12`, then its text. Returns how
    /// many comments went.
    ///
    /// # Errors
    ///
    /// [`CommentError::NoSuchComment`] for a review or a comment that is not there, and
    /// [`CommentError::NotYours`] for an agent's. Nothing changes.
    pub fn kill_review_comment(
        &mut self,
        review: ReviewId,
        comment: CommentId,
    ) -> Result<usize, CommentError> {
        let model = self.review(review).ok_or(CommentError::NoSuchComment)?;
        let root = model.comment(comment).ok_or(CommentError::NoSuchComment)?;
        if root.author() != Author::Person {
            return Err(CommentError::NotYours);
        }
        let mut text = String::new();
        for (index, each) in thread(model, root).into_iter().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            let who = match each.author() {
                Author::Person => "You",
                Author::Agent => "Agent",
            };
            text.push_str(&format!("{who} #{}\n", each.id().get()));
            text.push_str(each.text());
        }
        let removed = self
            .review_mut(review)
            .ok_or(CommentError::NoSuchComment)?
            .remove_comment(Author::Person, comment)?;
        self.push_kill(vec![Piece::from(Rope::from(text.as_str()))]);
        Ok(removed)
    }
}

/// `root` and every reply under it, at any depth, each after the one it replies to. Costs the
/// file's comments.
fn thread<'a>(model: &'a Review, root: &'a Comment) -> Vec<&'a Comment> {
    let mut replies: HashMap<CommentId, Vec<&Comment>> = HashMap::new();
    for comment in model.comments_on(root.path()) {
        if let Some(parent) = comment.reply_to() {
            replies.entry(parent).or_default().push(comment);
        }
    }
    let mut thread = vec![root];
    let mut at = 0;
    while let Some(parent) = thread.get(at).map(|comment| comment.id()) {
        thread.extend(replies.remove(&parent).unwrap_or_default());
        at += 1;
    }
    thread
}

/// What a draft's prompt says: what it is for, its path on one line and cut.
fn draft_label(draft: &DraftFor) -> String {
    match draft {
        DraftFor::Comment { path, anchor } => {
            let mut label = String::from("Comment on ");
            push_path(&mut label, path);
            match anchor {
                Anchor::Line { side, number } => {
                    let side = match side {
                        Side::Old => "old",
                        Side::New => "new",
                    };
                    label.push_str(&format!(" L{number} ({side})"));
                }
                Anchor::Hunk { index } => label.push_str(&format!(" hunk {}", index + 1)),
            }
            label
        }
        DraftFor::Reply { to } => format!("Reply to #{}", to.get()),
        DraftFor::Edit { comment } => format!("Edit #{}", comment.get()),
    }
}

/// Puts `path` on `label` as one line: a control or bidi formatting character as `?`, past
/// [`LABEL_PATH_CHARS`] cut with `…`.
fn push_path(label: &mut String, path: &[u8]) {
    let text = String::from_utf8_lossy(path);
    let mut chars = text.chars();
    for c in chars.by_ref().take(LABEL_PATH_CHARS) {
        let hidden = c.is_control()
            || matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
        label.push(if hidden { '?' } else { c });
    }
    if chars.next().is_some() {
        label.push_str(CUT);
    }
}

#[cfg(test)]
mod tests;
