//! The person's comments in a review buffer (ADR 0122, step 3): starting a draft for a line, a
//! hunk or a reply, editing, removing and resolving a comment, folding the agent's, moving by
//! comment, and keeping a draft.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state.
//! What a line is comes from the review's rows, never its text, so that no text of a file or of
//! a comment can pass for a line or a label.
//!
//! - Every command that writes a comment is `Person` (ADR 0122, Q11), so no command of the agent
//!   channel touches the person's comments; only `{` and `}`, which move, are `Look`.
//! - The comment commands act on the primary cursor, and refuse inside a prompt.
//! - What a draft holds is never lost: a draft that cannot be kept stays open, and `C-g` copies
//!   it to oso's kill ring, never to the clipboard. No comment's text reaches a message or the
//!   log.
//! - A draft whose buffer turned secret, as a yank of a secret entry turns it, is not kept: its
//!   text stays out of the review, whose comments the agent reads.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{DraftFor, ReviewId, ReviewRequest};
use oso_core::minibuffer::PromptError;
use oso_core::review::{Anchor, Author, CommentError, CommentId, MAX_COMMENT_BYTES, RowKind, Seek};

/// The commands of a review's comments.
pub const COMMANDS: &[Command] = &[
    person_command!(
        "review-comment",
        "Start a comment on the line or the hunk's header at point, or a reply to the comment at point.\n\nThe draft opens in the top float: `RET` breaks a line, `C-c C-c` keeps it, and `C-g` puts what it holds in the kill ring.",
        review_comment
    ),
    person_command!(
        "review-edit-comment",
        "Edit your comment at point in a draft; keeping it makes it unsent again.",
        review_edit_comment
    ),
    person_command!(
        "review-remove-comment",
        "Remove your comment at point with every reply under it, into the kill ring.\n\nIt asks nothing: `C-y` yanks the thread back as text.",
        review_remove_comment
    ),
    person_command!(
        "review-resolve",
        "Resolve the thread of the comment at point, or open it again if resolved.",
        review_resolve
    ),
    person_command!(
        "review-fold-agents",
        "Fold every agent comment of the review to its first line, or show them whole again.",
        review_fold_agents
    ),
    Command {
        name: "review-next-comment",
        doc: "Move every cursor to the label of the Nth next comment of the review.\n\nWith a negative N, move back.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: review_next_comment,
        state: None,
    },
    Command {
        name: "review-previous-comment",
        doc: "Move every cursor to the label of the Nth comment before it in the review.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: review_previous_comment,
        state: None,
    },
    person_command!(
        "review-keep-draft",
        "Keep the comment's draft: add it, the reply or the new text to the review, as yours.\n\nA draft whose line, hunk or comment is gone, that the review cannot take, or that is secret stays open: C-g puts it in the kill ring.",
        review_keep_draft
    ),
];

/// What `e` and `k` say of an agent's comment.
const NOT_YOURS: &str = "Not yours: c replies";

/// What the draft says when it is over [`MAX_COMMENT_BYTES`].
const TOO_LONG: &str = "Too long: a comment holds 64 KiB at most";

/// What a draft whose buffer may hold secrets says.
const SECRET: &str = "Secret text is not kept as a comment: C-g puts it in the kill ring";

/// What the line at the primary cursor of a review buffer is, for a comment.
enum Point {
    /// A line of a side.
    Line { path: Box<[u8]>, anchor: Anchor },
    /// A hunk's header.
    Hunk { path: Box<[u8]>, anchor: Anchor },
    /// A comment's label or text.
    Comment(CommentId),
    /// A file's header, a note or the title.
    Other,
}

/// The review shown and what the line at the primary cursor is, refused in a prompt and
/// outside a review.
fn point(ctx: &Ctx<'_>) -> Result<(ReviewId, Point), CommandError> {
    let review = super::current_review(ctx)?;
    let rows = ctx
        .editor
        .review_rows(review)
        .ok_or_else(|| CommandError::new("Not in a review"))?;
    let (window, buffer) = ctx.editor.current();
    let line = buffer
        .text()
        .offset_to_point(window.selections.primary().cursor)
        .row;
    let Some(row) = rows.at(line) else {
        return Ok((review, Point::Other));
    };
    let point = match row.kind {
        RowKind::Lines { .. } => match row.line() {
            Some((side, number)) => Point::Line {
                path: row.path.into(),
                anchor: Anchor::Line { side, number },
            },
            None => Point::Other,
        },
        RowKind::Hunk { hunk } => Point::Hunk {
            path: row.path.into(),
            anchor: Anchor::Hunk { index: hunk },
        },
        RowKind::Label { comment } | RowKind::Card { comment } => Point::Comment(comment),
        RowKind::File | RowKind::Note => Point::Other,
    };
    Ok((review, point))
}

/// The review shown and the comment at the primary cursor, refused elsewhere.
fn comment_at_point(ctx: &Ctx<'_>) -> Result<(ReviewId, CommentId), CommandError> {
    match point(ctx)? {
        (review, Point::Comment(comment)) => Ok((review, comment)),
        _ => Err(CommandError::new("Not on a comment")),
    }
}

/// Opens a draft for `draft` in `review`, holding `text`.
fn open(
    ctx: &mut Ctx<'_>,
    review: ReviewId,
    draft: DraftFor,
    text: &str,
) -> Result<(), CommandError> {
    ctx.editor
        .open_draft(review, draft, text)
        .map_err(|error| match error {
            // A draft opens over no other prompt.
            PromptError::TooDeep => CommandError::new("Close the open prompt first"),
            PromptError::Invalid => CommandError::new("The review is no longer open"),
            other => CommandError::new(other.to_string()),
        })?;
    ctx.request_redraw();
    Ok(())
}

/// Asks the host to draw the comments of the file at `path` of `review` again.
fn redraw(ctx: &mut Ctx<'_>, review: ReviewId, path: Box<[u8]>) {
    ctx.editor
        .request_review(ReviewRequest::Redraw { review, path });
    ctx.request_redraw();
}

/// The path of the file the comment `comment` of `review` is on.
fn path_of(ctx: &Ctx<'_>, review: ReviewId, comment: CommentId) -> Option<Box<[u8]>> {
    let model = ctx.editor.review(review)?;
    Some(model.comment(comment)?.path().into())
}

fn review_comment(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (review, point) = point(ctx)?;
    let draft = match point {
        Point::Line { path, anchor } | Point::Hunk { path, anchor } => {
            DraftFor::Comment { path, anchor }
        }
        // A reply to the comment at point (ADR 0122, Q15).
        Point::Comment(to) => DraftFor::Reply { to },
        Point::Other => return Err(CommandError::new("Comment on a line or a hunk's header")),
    };
    open(ctx, review, draft, "")
}

fn review_edit_comment(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (review, comment) = comment_at_point(ctx)?;
    let model = ctx.editor.review(review);
    let Some(found) = model.and_then(|model| model.comment(comment)) else {
        return Err(CommandError::new("No such comment"));
    };
    if found.author() != Author::Person {
        return Err(CommandError::new(NOT_YOURS));
    }
    let text = found.text().to_owned();
    open(ctx, review, DraftFor::Edit { comment }, &text)
}

fn review_remove_comment(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (review, comment) = comment_at_point(ctx)?;
    let path = path_of(ctx, review, comment);
    let removed = ctx
        .editor
        .kill_review_comment(review, comment)
        .map_err(|error| CommandError::new(refused(error)))?;
    let number = comment.get();
    let said = match removed.saturating_sub(1) {
        0 => format!("Removed #{number}: C-y yanks it"),
        1 => format!("Removed #{number} with 1 reply: C-y yanks it"),
        n => format!("Removed #{number} with {n} replies: C-y yanks it"),
    };
    ctx.editor.message(said);
    if let Some(path) = path {
        redraw(ctx, review, path);
    }
    Ok(())
}

fn review_resolve(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (review, comment) = comment_at_point(ctx)?;
    let Some(model) = ctx.editor.review(review) else {
        return Err(CommandError::new("No such comment"));
    };
    // The thread's root: up the replies, at most as many steps as there are comments.
    let mut root = model
        .comment(comment)
        .ok_or_else(|| CommandError::new("No such comment"))?;
    for _ in 0..oso_core::review::MAX_COMMENTS {
        match root.reply_to().and_then(|parent| model.comment(parent)) {
            Some(parent) => root = parent,
            None => break,
        }
    }
    let (id, resolved, path) = (root.id(), !root.is_resolved(), root.path().into());
    if let Some(model) = ctx.editor.review_mut(review) {
        model
            .resolve(Author::Person, id, resolved)
            .map_err(|error| CommandError::new(refused(error)))?;
    }
    let number = id.get();
    ctx.editor.message(if resolved {
        format!("Resolved #{number}")
    } else {
        format!("Reopened #{number}")
    });
    redraw(ctx, review, path);
    Ok(())
}

fn review_fold_agents(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let review = super::current_review(ctx)?;
    let folded = !ctx.editor.agents_folded(review);
    ctx.editor.fold_agents(review, folded);
    ctx.editor.message(if folded {
        "Agent comments folded"
    } else {
        "Agent comments shown whole"
    });
    ctx.request_redraw();
    Ok(())
}

fn review_next_comment(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let count = args.count();
    super::seek(ctx, Seek::Comment, count, comment_edge(count))
}

fn review_previous_comment(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let count = args.count().saturating_neg();
    super::seek(ctx, Seek::Comment, count, comment_edge(count))
}

/// What a motion by comment says when it stops short.
fn comment_edge(count: isize) -> &'static str {
    if count >= 0 {
        "No next comment"
    } else {
        "No comment before"
    }
}

/// What a refused change to a comment says.
fn refused(error: CommentError) -> &'static str {
    match error {
        CommentError::NoSuchFile | CommentError::NotInDiff | CommentError::NoSuchComment => {
            "Comment target gone"
        }
        CommentError::NotYours | CommentError::RepliesNotYours => NOT_YOURS,
        CommentError::TooLong => TOO_LONG,
        CommentError::TooMany => "The review holds as many comments as it can",
    }
}

/// What a draft the review refused says, as it stays open.
fn not_kept(error: CommentError) -> &'static str {
    match error {
        CommentError::NoSuchFile | CommentError::NotInDiff | CommentError::NoSuchComment => {
            "Comment target gone: C-g keeps the text in the kill ring"
        }
        CommentError::NotYours | CommentError::RepliesNotYours => {
            "Not yours: C-g keeps the text in the kill ring"
        }
        CommentError::TooLong => TOO_LONG,
        CommentError::TooMany => {
            "The review holds as many comments as it can: C-g keeps the text in the kill ring"
        }
    }
}

fn review_keep_draft(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let Some((review, draft)) = ctx
        .editor
        .draft()
        .map(|(review, draft)| (review, draft.clone()))
    else {
        return Err(CommandError::new("Not in a comment's draft"));
    };
    let buffer = ctx.editor.current().1;
    // The agent reads the review's comments, and a secret one would reach it.
    if buffer.is_secret() {
        ctx.editor.prompt_says(SECRET);
        ctx.request_redraw();
        return Ok(());
    }
    // Its length is looked at before its text is copied.
    if buffer.text().len() > MAX_COMMENT_BYTES {
        ctx.editor.prompt_says(TOO_LONG);
        ctx.request_redraw();
        return Ok(());
    }
    let text = buffer.text().to_string();
    if text.trim().is_empty() {
        ctx.editor.prompt_says("Type a comment, or C-g");
        ctx.request_redraw();
        return Ok(());
    }
    // The review takes it while the draft still holds it, so that what it refuses stays there.
    let kept =
        match ctx.editor.review_mut(review) {
            None => Err(CommentError::NoSuchFile),
            Some(model) => match &draft {
                DraftFor::Comment { path, anchor } => model
                    .add_comment(Author::Person, path, *anchor, text)
                    .map(|_| path.clone()),
                DraftFor::Reply { to } => model.reply(Author::Person, *to, text).map(|id| {
                    model
                        .comment(id)
                        .map_or_else(Box::default, |c| c.path().into())
                }),
                DraftFor::Edit { comment } => model
                    .edit_comment(Author::Person, *comment, text)
                    .map(|()| {
                        model
                            .comment(*comment)
                            .map_or_else(Box::default, |c| c.path().into())
                    }),
            },
        };
    match kept {
        Ok(path) => {
            ctx.request_redraw();
            ctx.editor
                .submit_prompt()
                .map_err(|error| CommandError::new(error.to_string()))?;
            // The answer is the comment's, which no command waits for.
            let _ = ctx.editor.take_prompt_outcome();
            redraw(ctx, review, path);
            Ok(())
        }
        Err(error) => {
            // The draft stays open with its text: `C-g` copies it to the kill ring alone, as an
            // entry that goes to no clipboard.
            ctx.editor.prompt_says(not_kept(error));
            ctx.request_redraw();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
