//! Stepping through a pull request's commits in its review (ADR 0167): `M-n` and `M-p` in a
//! pull request's review or one of its commits', and `l`, the commit list, a strip above the
//! review with a row for the whole pull request and a row a commit.
//!
//! Must not depend on: the host or its threads; a step asks the host, which opens a commit's
//! review on its own threads or shows the one open, and the list is laid out by the editor.
//!
//! - A step is `Person`: it opens a review, which runs git (ADR 0085). So is `l`: it selects
//!   the list, whose own keys run git or `gh`, which an agent must not leave under the
//!   person's next key.
//! - A step from the list keeps the list selected, as the host sees it selected: `SPC` asks
//!   from it, and `RET` selects the review below it first, so that the review shown is
//!   selected. `M-n` and `M-p` pass over a merge and a commit that changes nothing; `RET` on
//!   one shows it, a merge against its first parent.

use oso_core::buffer::BufferId;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{Editor, NoPair, ReviewId, ReviewRequest, Step};

/// Key sequences and the commands they run, in a pull request's commit list only.
pub const COMMITS_KEYS: &[(&str, &str)] = &[
    ("n", "next-line"),
    ("C-n", "next-line"),
    ("p", "previous-line"),
    ("C-p", "previous-line"),
    ("M-n", "review-next-commit"),
    ("M-p", "review-previous-commit"),
    ("RET", "review-commits-visit"),
    ("SPC", "review-commits-display"),
    ("g", "review-refresh"),
    ("l", "review-commits"),
    ("q", "review-commits"),
    ("?", "review-commits-keys"),
];

/// What the buffer `review-commits-keys` shows is named.
const KEYS_BUFFER: &str = "*Review commits keys*";

/// The commands of a pull request's commits.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "review-next-commit",
        doc: "Review the next commit of this pull request alone, in place of this review.\n\nFrom the whole pull request, its first commit. A merge and a commit that changes no file are stepped over. The commit's review is a review of one commit, with comments, viewed marks and a saved file of its own; the eight used last stay open, and one with comments not sent stays until they are sent. `C-c C-c` in any of them sends the comments of the whole pull request and of each commit, a group a commit.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: review_next_commit,
        state: None,
    },
    Command {
        name: "review-previous-commit",
        doc: "Review the commit before this one of this pull request alone, in place of this review.\n\nFrom its first commit, the whole pull request; from the whole, its last commit. A merge and a commit that changes no file are stepped over.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: review_previous_commit,
        state: None,
    },
    Command {
        name: "review-pull-request-whole",
        doc: "Review this pull request whole again, in place of the review of one of its commits.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: review_pull_request_whole,
        state: None,
    },
    Command {
        name: "review-commits",
        doc: "Show the pull request's commit list above the review and select it, or close the list from it.\n\nThe list has a row for the whole pull request, then, once its head moved since your last review of it (`review-checkpoint`), a row for what changed since, then a row a commit, oldest first: its place, its marks, its short id, its subject, its author and its date. `\u{25b6}` marks the review shown, `new` a commit new since your last review, `\u{2713}` a commit whose every hunk is viewed and `\u{25d0}` one with some, `\u{25cf}N` its comments of yours not sent, and `\u{2191}` a commit not pushed yet. A merge and a commit that changes no file are stepped over.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: review_commits,
        state: None,
    },
    Command {
        name: "review-commits-visit",
        doc: "Review the whole pull request, what changed since your last review or the commit at point, and select its review.\n\nWith the mark on another commit's row, review the commits from one to the other, as `r` reviews a range.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: review_commits_visit,
        state: None,
    },
    Command {
        name: "review-commits-display",
        doc: "Review the whole pull request, what changed since your last review or the commit at point, and stay in the commit list.\n\nWith the mark on another commit's row, review the commits from one to the other.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: review_commits_display,
        state: None,
    },
    Command {
        name: "review-checkpoint",
        doc: "Record this pull request's head as your last review of it.\n\nWhat comes after it then shows in the commit list as Since last review, its commits marked new, once the head moves. Pasting a pull request's comments with `C-c C-c` records it too. It is saved with the pull request's review.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: review_checkpoint,
        state: None,
    },
    Command {
        name: "review-commits-keys",
        doc: "Show the keys of a pull request's commit list, each with what its command does, in `*Review commits keys*`.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: review_commits_keys,
        state: None,
    },
];

/// The pull request review whose commit list `buffer` is.
pub(crate) fn commits_review(editor: &Editor, buffer: BufferId) -> Option<ReviewId> {
    editor
        .reviews()
        .map(|(id, _)| id)
        .find(|id| editor.review_commits_pane(*id) == Some(buffer))
}

/// The review the current buffer shows, or whose commit list or files pane it is.
fn review_here(ctx: &Ctx<'_>) -> Result<ReviewId, CommandError> {
    super::files::review_here(ctx)
}

/// The pull request review of `review`: itself, or the one it reviews a commit of.
fn pull_of(editor: &Editor, review: ReviewId) -> ReviewId {
    editor.review_pull(review).unwrap_or(review)
}

fn step(ctx: &mut Ctx<'_>, to: Step) -> Result<(), CommandError> {
    let review = review_here(ctx)?;
    ctx.editor
        .request_review(ReviewRequest::Step { review, to });
    ctx.request_redraw();
    Ok(())
}

fn review_next_commit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    step(ctx, Step::Next)
}

fn review_previous_commit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    step(ctx, Step::Previous)
}

fn review_pull_request_whole(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    step(ctx, Step::Whole)
}

fn review_commits(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let buffer = ctx.editor.window().buffer();
    if let Some(pull) = commits_review(ctx.editor, buffer) {
        // From the list: it closes, and the window below it, a review's, is selected again.
        ctx.editor.hide_review_commits(pull);
        ctx.request_redraw();
        return Ok(());
    }
    let review =
        (ctx.editor.review_of(buffer)).ok_or_else(|| CommandError::new("Not in a review"))?;
    let pull = pull_of(ctx.editor, review);
    if ctx.editor.review_commits(pull).is_none() {
        return Err(CommandError::new(
            "This review is no pull request's, or its commits are not listed yet",
        ));
    }
    let window = ctx.editor.window().id();
    let list = (ctx.editor.show_review_commits(pull, window)).map_err(|error| match error {
        NoPair::TooSmall => CommandError::new("No room for the commit list"),
        NoPair::NoBuffer | NoPair::Paired => CommandError::new("Not in a review"),
    })?;
    ctx.editor.select_window(list);
    ctx.request_redraw();
    Ok(())
}

/// The step the commit list's row at point names: the whole pull request on its first row,
/// what changed since the last review on the next while there is one (ADR 0167, step 2), a
/// commit on each after; or, with the mark on another commit's row, the commits from one to
/// the other.
fn row_step(ctx: &Ctx<'_>) -> Result<(ReviewId, Step), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let (window, buffer) = ctx.editor.current();
    let pull = commits_review(ctx.editor, window.buffer())
        .ok_or_else(|| CommandError::new("Not in a pull request's commit list"))?;
    let text = buffer.text();
    let primary = window.selections.primary();
    let line = text.offset_to_point(primary.cursor).row;
    let (count, since) = ctx
        .editor
        .review_commits(pull)
        .map_or((0, false), |commits| {
            (commits.entries.len(), commits.since.is_some())
        });
    let first = 1 + usize::from(since);
    let commit = |line: usize| (first..first + count).contains(&line).then(|| line - first);
    if let Some(mark) = primary.mark.map(|mark| text.offset_to_point(mark).row)
        && mark != line
    {
        return match (commit(mark), commit(line)) {
            (Some(a), Some(b)) => Ok((pull, Step::Range(a, b))),
            _ => Err(CommandError::new(
                "A range is of commits: set the mark on one commit's row, then go to another's",
            )),
        };
    }
    match line {
        0 => Ok((pull, Step::Whole)),
        1 if since => Ok((pull, Step::Since)),
        line => commit(line)
            .map(|at| (pull, Step::Commit(at)))
            .ok_or_else(|| CommandError::new("No commit here")),
    }
}

/// Drops the mark a range was asked with, so that the next `RET` names one row.
fn drop_mark(ctx: &mut Ctx<'_>, to: Step) {
    if matches!(to, Step::Range(..)) {
        ctx.editor.current_mut().0.selections.update(|selection| {
            selection.mark = None;
            Ok::<(), ()>(())
        });
    }
}

fn review_commits_visit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (review, to) = row_step(ctx)?;
    drop_mark(ctx, to);
    // The window of a review of the pull request, below the list, is selected first, so that
    // the host shows the review there selected.
    let below = (ctx.editor.windows())
        .find(|window| {
            ctx.editor.review_of(window.buffer()).is_some_and(|shown| {
                shown == review || ctx.editor.review_pull(shown) == Some(review)
            })
        })
        .map(|window| window.id());
    if let Some(below) = below {
        ctx.editor.select_window(below);
    }
    ctx.editor
        .request_review(ReviewRequest::Step { review, to });
    ctx.request_redraw();
    Ok(())
}

fn review_commits_display(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (review, to) = row_step(ctx)?;
    drop_mark(ctx, to);
    ctx.editor
        .request_review(ReviewRequest::Step { review, to });
    ctx.request_redraw();
    Ok(())
}

fn review_checkpoint(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let review = review_here(ctx)?;
    ctx.editor
        .request_review(ReviewRequest::Checkpoint { review });
    ctx.request_redraw();
    Ok(())
}

fn review_commits_keys(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let text = super::keys::keys_text(
        "Keys in a pull request's commit list",
        COMMITS_KEYS,
        "In a pull request's review or one of its commits', l shows the list, and M-n and M-p step through the commits.",
    );
    super::keys::show_keys(ctx, KEYS_BUFFER, &text)
}

#[cfg(test)]
#[path = "commits_tests.rs"]
mod tests;
