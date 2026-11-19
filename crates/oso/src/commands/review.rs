//! The commands of a review (ADR 0122): opening one of a scope from the review menu or `M-x`,
//! taking the session's baseline, and in a review buffer moving by hunk and file, reading the
//! files again, burying it, listing its keys and killing it. Its comments' commands are in
//! `comments`, and jumping to the source in `visit`.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state.
//! The review's keys are listed here, in [`KEYS`], which `bindings` binds, so that `?` says the
//! keys that are bound.
//!
//! - Opening asks the host ([`ReviewRequest::Open`]), which runs git on a worker for a trusted
//!   project only; every such command is `Person`, as it runs git (ADR 0085). `review-kill`
//!   is `Person` too, as it destroys the person's comments that were not sent.
//! - A motion works on every cursor, by the rows that say what each line is, never its text.

mod menu;

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{DraftFor, Editor, ReviewChoice, ReviewId, ReviewRequest, ScopeKind};
use oso_core::project::ProjectId;
use oso_core::review::{Author, Seek};

#[cfg(test)]
pub(crate) use menu::menu_heads;
pub use menu::review_menu;

/// The keys of a review buffer and their commands, which `bindings` binds in the review
/// keymap and `review-keys` lists.
pub const KEYS: &[(&str, &str)] = &[
    ("RET", "review-visit"),
    ("n", "review-next-hunk"),
    ("p", "review-previous-hunk"),
    ("N", "review-next-file"),
    ("P", "review-previous-file"),
    ("}", "review-next-comment"),
    ("{", "review-previous-comment"),
    ("[", "review-context-up"),
    ("]", "review-context-down"),
    ("=", "review-context-gap"),
    ("c", "review-comment"),
    ("e", "review-edit-comment"),
    ("k", "review-remove-comment"),
    ("r", "review-resolve"),
    ("a", "review-fold-agents"),
    ("A", "review-accept"),
    ("x", "review-reject"),
    ("v", "review-viewed"),
    ("u", "review-undo"),
    ("|", "review-toggle-panes"),
    ("s", "review-files"),
    ("/", "review-filter"),
    ("g", "review-refresh"),
    ("C-c C-c", "review-send"),
    ("q", "review-quit"),
    ("?", "review-keys"),
];

/// Why a review cannot open in a restricted project (ADR 0122, "Restricted projects").
pub(crate) const RESTRICTED: &str =
    "Reviewing runs git, which this project is not trusted to run: M-x trust-project";

/// The most characters of a revision, a branch or a project's name a review's name, title or
/// menu takes.
pub(crate) const MAX_NAME: usize = 80;

pub(crate) use crate::review::one_line;

/// The pull request `text` names: digits only, no sign, at most 18 of them, above 0.
pub(crate) fn pull_number(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.len() <= 18 && text.bytes().all(|b| b.is_ascii_digit());
    digits
        .then(|| text.parse().ok())
        .flatten()
        .filter(|&n| n > 0)
}

/// A command a person runs, with no argument.
macro_rules! person_command {
    ($name:literal, $doc:literal, $run:ident) => {
        Command {
            name: $name,
            doc: $doc,
            args: ArgSpec::None,
            reach: Reach::Person,
            run: $run,
            state: None,
        }
    };
}

mod comments;
mod context;
mod decide;
mod files;
mod keys;
mod saved;
mod send;
mod visit;

pub use comments::COMMANDS as COMMENT_COMMANDS;
pub use context::COMMANDS as CONTEXT_COMMANDS;
pub use decide::COMMANDS as DECISION_COMMANDS;
pub(crate) use decide::{open_buffer_of, place_of};
pub(crate) use files::prompt_text;
pub use files::{COMMANDS as FILES_COMMANDS, FILES_KEYS};
pub use saved::COMMANDS as SAVED_COMMANDS;
pub(crate) use saved::{SAVED_LIST, lost};
pub use send::{COMMANDS as SEND_COMMANDS, MESSAGE_KEYS};
pub use visit::COMMANDS as VISIT_COMMANDS;

/// The commands of reviews.
pub const COMMANDS: &[Command] = &[
    person_command!(
        "review-session",
        "Review what changed in the current buffer's project since its baseline.\n\nThe baseline is the work tree as `review-mark-baseline` (`B` in the review menu) took it, untracked files included. git runs only in a trusted project, on a worker.",
        review_session
    ),
    person_command!(
        "review-uncommitted",
        "Review the current buffer's project's changes against `HEAD`, untracked files included.\n\ngit runs only in a trusted project, on a worker. The review buffer opens in the selected window and fills as each file is read.",
        review_uncommitted
    ),
    person_command!(
        "review-unstaged",
        "Review the current buffer's project's changes that are not staged: the work tree's tracked files against the index.",
        review_unstaged
    ),
    person_command!(
        "review-staged",
        "Review the current buffer's project's staged changes: the index against `HEAD`.",
        review_staged
    ),
    Command {
        name: "review-branch",
        doc: "Review the current buffer's project's branch against its base, untracked files included.\n\nThe base is the branch's upstream, unless that is the same branch on a remote, else the branch `origin/HEAD` names, else `main` or `master`: as the review menu found it when it opened, or found first. With `C-u`, it asks for the base. The old side is where the branch left the base (`git merge-base`).",
        args: ArgSpec::TextOnPrefix {
            prompt: "Branch base: ",
            history: "review-base",
        },
        reach: Reach::Person,
        run: review_branch,
        state: None,
    },
    Command {
        name: "review-commit",
        doc: "Review one commit of the current buffer's project, read in a prompt, against its first parent.",
        args: ArgSpec::Text {
            prompt: "Review commit: ",
            history: "review-revision",
        },
        reach: Reach::Person,
        run: review_commit,
        state: None,
    },
    Command {
        name: "review-range",
        doc: "Review a range of commits of the current buffer's project, read in a prompt as FROM..TO.\n\nThe old side is FROM's first parent and the new side TO, so FROM's own changes are in the review.",
        args: ArgSpec::Text {
            prompt: "Review range (FROM..TO): ",
            history: "review-revision",
        },
        reach: Reach::Person,
        run: review_range,
        state: None,
    },
    Command {
        name: "review-pull-request",
        doc: "Review a pull request of the current buffer's project's GitHub repository, its number read in a prompt.\n\n`gh` says its base and head, and their commits are fetched from the remote when this clone lacks them, as objects alone: no ref moves, and `FETCH_HEAD` stays as it was. The old side is where the head left the base (`git merge-base`), as GitHub's \"Files changed\" shows it; the new side is its head, or the work tree when its head branch is the one checked out and holds its head. `g` asks `gh` again. Comments go to the session whose worktree has its head branch checked out. git and `gh` run only in a trusted project, on a worker.",
        args: ArgSpec::Text {
            prompt: "Review pull request #",
            history: "review-pull-request",
        },
        reach: Reach::Person,
        run: review_pull_request,
        state: None,
    },
    person_command!(
        "review-mark-baseline",
        "Take a baseline of the current buffer's project's work tree, for `review-session`.\n\nIt writes the work tree, untracked files included, as a tree that nothing references, as `git add` does; git keeps it for its prune grace period. git runs only in a trusted project, on a worker.",
        review_mark_baseline
    ),
    Command {
        name: "review-next-hunk",
        doc: "Move every cursor to the header of the Nth next hunk of the review.\n\nWith a negative N, move back. A file's header counts as no hunk.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: review_next_hunk,
        state: None,
    },
    Command {
        name: "review-previous-hunk",
        doc: "Move every cursor to the header of the Nth hunk before it in the review.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: review_previous_hunk,
        state: None,
    },
    Command {
        name: "review-next-file",
        doc: "Move every cursor to the header of the Nth next file of the review.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: review_next_file,
        state: None,
    },
    Command {
        name: "review-previous-file",
        doc: "Move every cursor to the header of the Nth file before it in the review.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: review_previous_file,
        state: None,
    },
    person_command!(
        "review-refresh",
        "Read the review's files again now, the files in view first.\n\nOnly files whose content changed are read again; every cursor keeps its place outside the files that changed.",
        review_refresh
    ),
    Command {
        name: "review-quit",
        doc: "Bury the review buffer: show the buffer shown before it in this window.\n\nThe review stays open; `C-x k` closes it.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: review_quit,
        state: None,
    },
    Command {
        name: "review-toggle-panes",
        doc: "Show the review in two panes, its old side and its new side lined up row for row, or as one stream again.\n\nThe panes go side by side when the review's area is 160 columns wide or more, and stacked below 150. Every window showing the review follows the choice. With a prefix argument, the review shows as each area's width has it again, as it does at first: two panes side by side from 160 columns, one stream below 150.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: review_toggle_panes,
        state: None,
    },
    Command {
        name: "review-keys",
        doc: "Show the keys of a review buffer, each with what its command does, in `*Review keys*`.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: keys::review_keys,
        state: None,
    },
    person_command!(
        "review-kill",
        "Close the current review: its buffer and its workers.\n\nThe review is saved as it changes, so its comments, decisions and viewed marks come back when the same scope of the project opens again, or with `review-resume`. It asks first only for what would be lost: an open draft, or comments of yours not sent when the review could not be saved. `C-x k` in a review buffer runs it.",
        review_kill
    ),
];

/// The project reviews start from: the review's when the selected window shows a review
/// buffer, which has no project of its own, else the project of the buffer it shows.
pub(crate) fn review_project(editor: &Editor) -> Option<ProjectId> {
    let buffer = editor.tiled().0.buffer();
    match editor.review_of(buffer) {
        Some(review) => editor
            .reviews()
            .find_map(|(id, project)| (id == review).then_some(project)),
        None => editor.buffer(buffer)?.project(),
    }
}

/// The project reviews start from ([`review_project`]), refused in a prompt or outside a
/// project.
fn project(ctx: &Ctx<'_>) -> Result<ProjectId, CommandError> {
    super::not_in_prompt(ctx)?;
    review_project(ctx.editor).ok_or_else(|| CommandError::new("Not in a git repository"))
}

/// Asks the host to open a review of `scope`.
fn open(ctx: &mut Ctx<'_>, scope: ScopeKind, revision: Option<String>) -> Result<(), CommandError> {
    let project = project(ctx)?;
    if !ctx.editor.project_trusted(project) {
        return Err(CommandError::new(RESTRICTED));
    }
    ctx.editor.request_review(ReviewRequest::Open {
        project,
        scope,
        revision,
    });
    ctx.request_redraw();
    Ok(())
}

fn review_session(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let project = project(ctx)?;
    let taken = ctx
        .editor
        .scope_menu(project)
        .and_then(|menu| menu.baseline_taken());
    if taken.is_none() {
        return Err(CommandError::new("No baseline: B takes one"));
    }
    open(ctx, ScopeKind::Session, None)
}

fn review_uncommitted(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    open(ctx, ScopeKind::Uncommitted, None)
}

fn review_unstaged(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    open(ctx, ScopeKind::Unstaged, None)
}

fn review_staged(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    open(ctx, ScopeKind::Staged, None)
}

fn review_branch(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let typed = ctx.answer().map(str::trim).filter(|base| !base.is_empty());
    let base = match typed {
        Some(base) => Some(base.to_owned()),
        // The base the menu found, or none, which the host finds first.
        None => {
            let project = project(ctx)?;
            let found = ctx.editor.scope_menu(project).and_then(|menu| menu.base());
            found.map(str::to_owned)
        }
    };
    open(ctx, ScopeKind::Branch, base)
}

fn review_commit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let revision = ctx.answer().unwrap_or_default().trim().to_owned();
    if revision.is_empty() {
        return Err(CommandError::new("No commit was named"));
    }
    open(ctx, ScopeKind::Commit, Some(revision))
}

fn review_range(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let range = ctx.answer().unwrap_or_default().trim().to_owned();
    match range.split_once("..") {
        Some((from, to)) if !from.is_empty() && !to.is_empty() && !to.starts_with('.') => {
            open(ctx, ScopeKind::Range, Some(range))
        }
        _ => Err(CommandError::new("A range is FROM..TO")),
    }
}

fn review_pull_request(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let answer = ctx.answer().unwrap_or_default().trim();
    let number = pull_number(answer.strip_prefix('#').unwrap_or(answer))
        .ok_or_else(|| CommandError::new("A pull request is named by its number"))?;
    open(ctx, ScopeKind::PullRequest, Some(number.to_string()))
}

fn review_mark_baseline(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let project = project(ctx)?;
    if !ctx.editor.project_trusted(project) {
        return Err(CommandError::new(RESTRICTED));
    }
    ctx.editor
        .request_review(ReviewRequest::Baseline { project });
    ctx.editor.message("Taking a baseline\u{2026}");
    ctx.request_redraw();
    Ok(())
}

/// The review the current buffer shows, refused elsewhere.
fn current_review(ctx: &Ctx<'_>) -> Result<ReviewId, CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor
        .review_of(ctx.editor.window().buffer())
        .ok_or_else(|| CommandError::new("Not in a review"))
}

/// Moves every cursor `count` times to the next row `to` seeks, back for a negative count, as
/// far as each goes, failing with `edge` if one stopped short.
fn seek(ctx: &mut Ctx<'_>, to: Seek, count: isize, edge: &str) -> Result<(), CommandError> {
    current_review(ctx)?;
    // A clone of the rows is O(1): they are shared. A pane has rows of its own.
    let Some(rows) = ctx
        .editor
        .review_rows_of(ctx.editor.window().buffer())
        .cloned()
    else {
        return Err(CommandError::new("Not in a review"));
    };
    seek_in(ctx, &rows, to, count, edge)
}

/// [`seek`] by `rows`, those of the selected window's buffer.
fn seek_in(
    ctx: &mut Ctx<'_>,
    rows: &oso_core::review::Rows,
    to: Seek,
    count: isize,
    edge: &str,
) -> Result<(), CommandError> {
    let forward = count >= 0;
    let steps = count.unsigned_abs();
    let (window, buffer) = ctx.editor.current_mut();
    let text = buffer.text().clone();
    let mut short = false;
    let update = window.selections.update(|selection| {
        let mut line = text.offset_to_point(selection.cursor).row;
        for _ in 0..steps {
            match rows.seek(line, to, forward) {
                Some(next) => line = next,
                None => {
                    short = true;
                    break;
                }
            }
        }
        selection.move_to(text.line_start(line));
        Ok::<(), ()>(())
    });
    if update.changed {
        ctx.request_redraw();
    }
    if short {
        return Err(CommandError::new(edge.to_owned()));
    }
    Ok(())
}

fn review_next_hunk(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let count = args.count();
    seek(
        ctx,
        Seek::Hunk,
        count,
        if count >= 0 {
            "No next hunk"
        } else {
            "No hunk before"
        },
    )
}

fn review_previous_hunk(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let count = args.count().saturating_neg();
    seek(
        ctx,
        Seek::Hunk,
        count,
        if count >= 0 {
            "No next hunk"
        } else {
            "No hunk before"
        },
    )
}

fn review_next_file(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let count = args.count();
    seek(
        ctx,
        Seek::File,
        count,
        if count >= 0 {
            "No next file"
        } else {
            "No file before"
        },
    )
}

fn review_previous_file(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let count = args.count().saturating_neg();
    seek(
        ctx,
        Seek::File,
        count,
        if count >= 0 {
            "No next file"
        } else {
            "No file before"
        },
    )
}

fn review_refresh(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    // From the files pane too (step 6b).
    let review = files::review_here(ctx)?;
    ctx.editor.request_review(ReviewRequest::Refresh(review));
    ctx.editor
        .message("Reading the review's files again\u{2026}");
    ctx.request_redraw();
    Ok(())
}

fn review_toggle_panes(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let review = current_review(ctx)?;
    // `C-u |`: as each area's width has it again.
    if args.raw().is_some() {
        ctx.editor.choose_review_layout(review, ReviewChoice::Auto);
        ctx.editor.message("Two panes or one by the width");
        ctx.request_redraw();
        return Ok(());
    }
    let buffer = ctx.editor.window().buffer();
    let in_panes =
        (ctx.editor.review_panes(review)).is_some_and(|(old, new)| buffer == old || buffer == new);
    // The choice turns, and what shows only says which way while it is automatic.
    let choice = match ctx.editor.review_choice(review) {
        ReviewChoice::Panes => ReviewChoice::Unified,
        ReviewChoice::Unified => ReviewChoice::Panes,
        ReviewChoice::Auto if in_panes => ReviewChoice::Unified,
        ReviewChoice::Auto => ReviewChoice::Panes,
    };
    ctx.editor.choose_review_layout(review, choice);
    ctx.request_redraw();
    Ok(())
}

fn review_quit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    current_review(ctx)?;
    // A pane buries its pair: the review's one window goes back, as the review's buffer would.
    let window = ctx.editor.window().id();
    ctx.editor.show_review_unified(window);
    ctx.editor.walk_buffers(1).map_err(CommandError::new)?;
    ctx.request_redraw();
    Ok(())
}

/// How many comments of the person's in the review `id` were not sent, an open draft for it
/// counted as one, unless it edits a comment counted already.
pub(crate) fn unsent(editor: &Editor, id: ReviewId) -> usize {
    let Some(review) = editor.review(id) else {
        return 0;
    };
    let unsent = |comment: &oso_core::review::Comment| {
        comment.author() == Author::Person && !comment.is_sent()
    };
    let drafted = match editor.draft() {
        Some((drafted, DraftFor::Edit { comment })) if drafted == id => {
            !review.comment(*comment).is_some_and(unsent)
        }
        Some((drafted, _)) => drafted == id,
        None => false,
    };
    review.comments().filter(|comment| unsent(comment)).count() + usize::from(drafted)
}

/// How many comments of the person's every open review would lose if the editor went now
/// ([`lost`]): its open draft, and those not sent of a review the host could not save.
pub(crate) fn unsent_everywhere(editor: &Editor) -> usize {
    editor.reviews().map(|(id, _)| lost(editor, id)).sum()
}

fn review_kill(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let review = current_review(ctx)?;
    kill(ctx, review)
}

/// Closes the review `review`, asking first while it would lose comments of the person's
/// ([`lost`]), until a `y` to that question runs `review-kill` again. The review asked about is
/// shown in the selected window first, so that the `y`, which kills the review shown, kills
/// this one. Comments the saved review keeps are said to be kept (step 6, Q-G).
pub(crate) fn kill(ctx: &mut Ctx<'_>, review: ReviewId) -> Result<(), CommandError> {
    let unsent = lost(ctx.editor, review);
    let kept = if ctx.editor.review_kept(review) {
        self::unsent(ctx.editor, review) - unsent
    } else {
        0
    };
    if unsent > 0 && !ctx.confirmed() {
        let (window, shown) = {
            let window = ctx.editor.tiled().0;
            (window.id(), ctx.editor.review_of(window.buffer()))
        };
        if shown != Some(review)
            && let Some(buffer) = ctx.editor.review_buffer(review)
        {
            ctx.editor.show_in(window, buffer);
        }
        let question = format!(
            "{unsent} comment{} of yours will be lost; kill?",
            if unsent == 1 { "" } else { "s" }
        );
        return super::session::ask(ctx, question, "review-kill", ["kill", "keep"]);
    }
    ctx.editor.remove_review(review);
    if kept > 0 {
        let comments = if kept == 1 { "comment" } else { "comments" };
        ctx.editor.message(format!(
            "{kept} unsent {comments} kept; review-resume brings them back"
        ));
    }
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod pr_tests;
#[cfg(test)]
pub(crate) mod tests;
