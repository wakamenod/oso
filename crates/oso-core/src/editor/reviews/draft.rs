//! A review comment's draft and the person's removal of a comment (ADR 0122, step 3): the
//! draft is a prompt in the top float that remembers what it is for, whose answer and window
//! take as many rows as its window's wrap makes, and what it holds goes to the kill ring when
//! it is let go, as a removed thread does.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads; the host builds the
//! review's text again when asked.
//!
//! - A draft opens only with no prompt open, so there is at most one, and review texts wait
//!   while it is open, so the line it is on cannot move under it.
//! - What it is for is forgotten when its prompt closes, however it closes.
//! - A removed thread's text is bounded, and one an agent wrote in stays in oso's kill ring.

use super::{Editor, ReviewId};
use crate::buffer::{BufferId, Replacement};
use crate::display_map::cell::is_invisible;
use crate::kill::{Direction, Piece};
use crate::minibuffer::{self, CUT, Kind, PromptError, PromptId, PromptLayout};
use crate::review::{Anchor, Author, CommentError, CommentId, Side};
use crate::rope::Rope;
use crate::window::WindowId;

/// The most characters of a path a draft's label shows.
const LABEL_PATH_CHARS: usize = 80;

/// The most bytes of text a removed thread puts in the kill ring; the comments past it are
/// counted, not copied (ADR 0122, Q-H).
pub(crate) const MAX_KILLED_THREAD: usize = 1024 * 1024;

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

/// The open draft: its prompt and the window it is typed in, its review and what it is for.
#[derive(Debug)]
pub(in crate::editor) struct Draft {
    prompt: PromptId,
    window: WindowId,
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
    /// [`PromptError::TooDeep`] while any prompt is open, of any kind and at any depth, as a
    /// draft never opens in or over another prompt: the caller says to close it first.
    /// [`PromptError::Invalid`] for a review that is not open, as one closed meanwhile. Nothing
    /// changes either way.
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
        let window = self.current().0.id();
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
            window,
            review,
            draft,
        });
        self.relayout_prompt();
        Ok(prompt)
    }

    /// The innermost prompt's draft: its review and what it is for. `None` when the innermost
    /// prompt is not a draft.
    pub fn draft(&self) -> Option<(ReviewId, &DraftFor)> {
        let draft = self.innermost_draft()?;
        Some((draft.review, &draft.draft))
    }

    /// The open draft, when its prompt is the innermost.
    fn innermost_draft(&self) -> Option<&Draft> {
        let draft = self.reviews.draft.as_ref()?;
        (self.prompt()?.id() == draft.prompt).then_some(draft)
    }

    /// Whether the window at `index` is the innermost draft's, which wraps as a tiled window
    /// does.
    pub(in crate::editor) fn is_draft_window(&self, index: usize) -> bool {
        self.innermost_draft().is_some_and(|draft| {
            (self.windows.get(index)).is_some_and(|window| window.id() == draft.window)
        })
    }

    /// Where the innermost prompt's parts go as a draft labelled `label`: its answer as many
    /// rows as its window wraps its text into at the answer's width, up to 10 (ADR 0122).
    pub(in crate::editor) fn draft_prompt_layout(
        &self,
        label: &str,
        message: Option<&str>,
    ) -> PromptLayout {
        let columns = minibuffer::layout(&self.frame, label, Kind::Draft, message)
            .input
            .columns;
        let rows = self
            .innermost_draft()
            .and_then(|_| self.buffer(self.prompt()?.buffer))
            .map_or(1, |buffer| {
                minibuffer::draft_rows(&buffer.display(), columns)
            });
        minibuffer::draft_layout(&self.frame, label, message, rows)
    }

    /// The rows of the innermost prompt's window under `layout`: its answer's for a draft, one
    /// for any other prompt.
    pub(in crate::editor) fn draft_window_rows(&self, layout: &PromptLayout) -> usize {
        match self.draft() {
            Some(_) => layout.input.rows.max(1),
            None => 1,
        }
    }

    /// Keeps the cursor of a draft's window in view once it is sized: it wraps at its width.
    pub(in crate::editor) fn follow_draft(&mut self) {
        if self.draft().is_some() {
            self.follow_window(self.selected);
        }
    }

    /// Lays the draft out again when its text changed, as its answer may take more rows.
    pub(in crate::editor) fn draft_edited(&mut self, buffer: BufferId) {
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
    /// it, as a secret entry when the draft holds secret text.
    pub(in crate::editor) fn kill_draft_text(&mut self) {
        if self.draft().is_none() {
            return;
        }
        let Some(buffer) = self.prompt().map(|prompt| prompt.buffer) else {
            return;
        };
        let len = self.buffer(buffer).map_or(0, |shown| shown.text().len());
        if len > 0 {
            let _ = self.kill_in(buffer, std::iter::once(0..len), Direction::Forward, true);
        }
    }

    /// Removes the person's own comment `comment` from the review `review`, with every reply
    /// under it, and puts the thread in the kill ring: each comment as its label line,
    /// `You  #7` or `Agent  #12`, then its text, up to 1 MiB, past which a last line counts the
    /// comments left out (Q-H). It goes to the clipboard as a kill does, unless an agent wrote
    /// in the thread: then it stays in oso's kill ring (Q-G). Returns how many comments went.
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
        let thread = model.thread(root.id()).ok_or(CommentError::NoSuchComment)?;
        let comments = thread.iter().filter_map(|id| model.comment(*id));
        let agents = comments.clone().any(|each| each.author() == Author::Agent);
        let mut text = String::new();
        for (index, each) in comments.enumerate() {
            let who = match each.author() {
                Author::Person => "You",
                Author::Agent => "Agent",
            };
            let label = format!("{who}  #{}\n", each.id().get());
            // Sizes are looked at before the text is copied.
            let separator = usize::from(index > 0);
            if text.len() + separator + label.len() + each.text().len() > MAX_KILLED_THREAD {
                let left = thread.len() - index;
                let noun = if left == 1 { "comment" } else { "comments" };
                text.push_str(&format!("\n\u{2026} {left} more {noun} not copied"));
                break;
            }
            if index > 0 {
                text.push('\n');
            }
            text.push_str(&label);
            text.push_str(each.text());
        }
        let removed = self
            .review_mut(review)
            .ok_or(CommentError::NoSuchComment)?
            .remove_comment(Author::Person, comment)?;
        self.push_kill(vec![Piece::from(Rope::from(text.as_str()))], !agents);
        Ok(removed)
    }
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

/// Puts `path` on `label` as one line: a control character, or one the display map draws as
/// a boxed code point (a bidi control or a default-ignorable one), as `?`, past
/// [`LABEL_PATH_CHARS`] cut with `…`.
fn push_path(label: &mut String, path: &[u8]) {
    let text = String::from_utf8_lossy(path);
    let mut chars = text.chars();
    for c in chars.by_ref().take(LABEL_PATH_CHARS) {
        let hidden = c.is_control() || is_invisible(c);
        label.push(if hidden { '?' } else { c });
    }
    if chars.next().is_some() {
        label.push_str(CUT);
    }
}

#[cfg(test)]
mod tests;
