//! The commands of a review (ADR 0122): opening one of a scope from the review menu or `M-x`,
//! taking the session's baseline, and in a review buffer moving by hunk and file, reading the
//! files again, burying it, listing its keys and killing it. Its comments' commands are in
//! `comments`.
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

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{DraftFor, Editor, ReviewId, ReviewRequest, ScopeKind};
use oso_core::project::ProjectId;
use oso_core::review::{Author, Seek};

#[cfg(test)]
pub(crate) use menu::menu_heads;
pub use menu::review_menu;

/// The keys of a review buffer and their commands, which `bindings` binds in the review
/// keymap and `review-keys` lists.
pub const KEYS: &[(&str, &str)] = &[
    ("n", "review-next-hunk"),
    ("p", "review-previous-hunk"),
    ("N", "review-next-file"),
    ("P", "review-previous-file"),
    ("}", "review-next-comment"),
    ("{", "review-previous-comment"),
    ("c", "review-comment"),
    ("e", "review-edit-comment"),
    ("k", "review-remove-comment"),
    ("r", "review-resolve"),
    ("a", "review-fold-agents"),
    ("g", "review-refresh"),
    ("q", "review-quit"),
    ("?", "review-keys"),
];

/// What the buffer `review-keys` shows is named.
const KEYS_BUFFER: &str = "*Review keys*";

/// Why a review cannot open in a restricted project (ADR 0122, "Restricted projects").
pub(crate) const RESTRICTED: &str =
    "Reviewing runs git, which this project is not trusted to run: M-x trust-project";

/// The most characters of a revision, a branch or a project's name a review's name, title or
/// menu takes.
pub(crate) const MAX_NAME: usize = 80;

/// `text` on one line, at most `max` characters, with no control character.
pub(crate) fn one_line(text: &str, max: usize) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    if text.chars().nth(max).is_some() {
        out.push('\u{2026}');
    }
    out
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

pub use comments::COMMANDS as COMMENT_COMMANDS;

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
        name: "review-keys",
        doc: "Show the keys of a review buffer, each with what its command does, in `*Review keys*`.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: review_keys,
        state: None,
    },
    person_command!(
        "review-kill",
        "Close the current review: its buffer and its workers.\n\nIt asks first while the review holds comments of yours that were not sent, which are lost with it. `C-x k` in a review buffer runs it.",
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
    let review = current_review(ctx)?;
    // A clone of the rows is O(1): they are shared.
    let Some(rows) = ctx.editor.review_rows(review).cloned() else {
        return Err(CommandError::new("Not in a review"));
    };
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
    let review = current_review(ctx)?;
    ctx.editor.request_review(ReviewRequest::Refresh(review));
    ctx.editor
        .message("Reading the review's files again\u{2026}");
    ctx.request_redraw();
    Ok(())
}

fn review_quit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    current_review(ctx)?;
    ctx.editor.walk_buffers(1).map_err(CommandError::new)?;
    ctx.request_redraw();
    Ok(())
}

fn review_keys(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let text = keys_text();
    let shown = ctx
        .editor
        .buffer_named(KEYS_BUFFER)
        .filter(|id| ctx.editor.buffer(*id).is_some_and(|b| b.is_internal()));
    let id = match shown {
        Some(id) => id,
        None => ctx
            .editor
            .add_internal_buffer(KEYS_BUFFER, Mode::Fundamental),
    };
    ctx.editor.set_internal_text(id, &text);
    let window = ctx.editor.window().id();
    ctx.editor.show_in(window, id);
    ctx.request_redraw();
    Ok(())
}

/// What `*Review keys*` says: each key, and its command's name and summary.
fn keys_text() -> String {
    let mut text = String::from("Keys in a review buffer\n\n");
    for (keys, name) in KEYS {
        let summary = COMMANDS
            .iter()
            .chain(COMMENT_COMMANDS)
            .find(|command| command.name == *name)
            .and_then(|command| command.doc.lines().next())
            .unwrap_or_default();
        text.push_str(&format!("{keys:<6}{name:<24}{summary}\n"));
    }
    text.push_str("\nC-x v r opens the review menu; C-x k closes the review.\n");
    text
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

/// How many comments of the person's every open review holds that were not sent.
pub(crate) fn unsent_everywhere(editor: &Editor) -> usize {
    editor.reviews().map(|(id, _)| unsent(editor, id)).sum()
}

fn review_kill(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let review = current_review(ctx)?;
    kill(ctx, review)
}

/// Closes the review `review`, asking first while it holds unsent comments of the person's,
/// until a `y` to that question runs `review-kill` again. The review asked about is shown in
/// the selected window first, so that the `y`, which kills the review shown, kills this one.
pub(crate) fn kill(ctx: &mut Ctx<'_>, review: ReviewId) -> Result<(), CommandError> {
    let unsent = unsent(ctx.editor, review);
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
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
