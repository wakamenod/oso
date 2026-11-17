//! A review's files pane and its filter (ADR 0122, step 6b): the keys and commands of the
//! narrow window at the left of a review that lists its files, and `/`, which shows only the
//! files whose path or comments hold every word typed.
//!
//! Must not depend on: the host or its threads; the pane's text comes from the review's view
//! thread, and the commands change the editor only.
//!
//! - `s` in a review shows its files pane and selects it; `s` or `q` in the pane closes it.
//! - `RET` and `SPC` put the header of the file at point at the top of each window showing the
//!   review, `RET` selecting the first; nothing is decided from the pane (Q-A).
//! - `review-filter` is `Person`: what a filter hides is the person's choice of what to read,
//!   and its prompt opens with the filter's words in it.

use oso_core::buffer::BufferId;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{Editor, NoPair, ReviewId};
use oso_core::review::{Filter, RowKind, Seek};
use oso_core::selection::SelectionSet;
use oso_core::window::{Scroll, WindowId};

/// Key sequences and the commands they run, in a review's files pane only.
pub const FILES_KEYS: &[(&str, &str)] = &[
    ("n", "review-files-next"),
    ("C-n", "review-files-next"),
    ("p", "review-files-previous"),
    ("C-p", "review-files-previous"),
    ("RET", "review-files-visit"),
    ("SPC", "review-files-display"),
    ("/", "review-filter"),
    ("g", "review-refresh"),
    ("s", "review-files"),
    ("q", "review-files"),
    ("?", "review-files-keys"),
];

/// What the buffer `review-files-keys` shows is named.
const KEYS_BUFFER: &str = "*Review files keys*";

/// The command whose prompt opens holding the review's filter.
const FILTER: &str = "review-filter";

/// The commands of the files pane and the filter.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "review-files",
        doc: "Show the review's files pane at its left and select it, or close the pane from it.\n\nThe pane lists the files the review shows, one row each: its status, the lines put in and taken out, its comments as yours\u{b7}the agent's, `!` when one is outdated, `\u{2022}` while an agent's is unread, `\u{2713}` when every hunk is decided or viewed and `\u{25d0}` when some are, then its path. Its cursor follows the review's file.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: review_files,
        state: None,
    },
    Command {
        name: "review-files-next",
        doc: "Move the files pane's cursor to the Nth next file.\n\nWith a negative N, move back.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: review_files_next,
        state: None,
    },
    Command {
        name: "review-files-previous",
        doc: "Move the files pane's cursor to the Nth file before it.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: review_files_previous,
        state: None,
    },
    Command {
        name: "review-files-visit",
        doc: "Show the file at point in the review, its header at the top, and select the review.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: review_files_visit,
        state: None,
    },
    Command {
        name: "review-files-display",
        doc: "Show the file at point in the review, its header at the top, and stay in the files pane.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: review_files_display,
        state: None,
    },
    Command {
        name: FILTER,
        doc: "Show only the review's files that every word typed matches, by its path or a comment's text.\n\nWords are separated by spaces, and match case-insensitively unless one holds an upper-case letter. A renamed file matches by its old path too; a file that may hold secrets matches by its path alone. The prompt opens holding the filter's words; an empty answer shows every file again. The review's title says how many files the filter hides.",
        args: ArgSpec::Text {
            prompt: "Filter files: ",
            history: "review-filter",
        },
        reach: Reach::Person,
        run: review_filter,
        state: None,
    },
    Command {
        name: "review-files-keys",
        doc: "Show the keys of a review's files pane, each with what its command does, in `*Review files keys*`.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: review_files_keys,
        state: None,
    },
];

/// The review whose files pane `buffer` is.
fn files_review(editor: &Editor, buffer: BufferId) -> Option<ReviewId> {
    editor
        .reviews()
        .map(|(id, _)| id)
        .find(|id| editor.review_files(*id) == Some(buffer))
}

/// The review the current buffer shows, or whose files pane it is; refused elsewhere.
pub(super) fn review_here(ctx: &Ctx<'_>) -> Result<ReviewId, CommandError> {
    super::super::not_in_prompt(ctx)?;
    review_shown(ctx.editor, ctx.editor.window().buffer())
        .ok_or_else(|| CommandError::new("Not in a review"))
}

/// The review `buffer` shows, or whose files pane it is.
fn review_shown(editor: &Editor, buffer: BufferId) -> Option<ReviewId> {
    (editor.review_of(buffer)).or_else(|| files_review(editor, buffer))
}

/// The review whose files pane the current buffer is, refused elsewhere.
fn current_pane(ctx: &Ctx<'_>) -> Result<ReviewId, CommandError> {
    super::super::not_in_prompt(ctx)?;
    files_review(ctx.editor, ctx.editor.window().buffer())
        .ok_or_else(|| CommandError::new("Not in a review's files pane"))
}

/// The text the prompt of the command `name` opens with: the filter of the review the selected
/// window shows, for `review-filter`.
pub(crate) fn prompt_text(editor: &Editor, name: &str) -> Option<String> {
    if name != FILTER {
        return None;
    }
    let review = review_shown(editor, editor.tiled().0.buffer())?;
    Some(editor.review_filter(review)?.text().to_owned())
}

/// The windows showing the review `id` or a pane of its pair, the review buffer's
/// first, then the new side's and the old side's.
fn review_windows(editor: &Editor, id: ReviewId) -> Vec<WindowId> {
    let review = editor.review_buffer(id);
    let (old, new) = editor.review_panes(id).unzip();
    let mut windows = Vec::new();
    for buffer in [review, new, old].into_iter().flatten() {
        windows.extend(
            (editor.windows())
                .filter(|window| window.buffer() == buffer)
                .map(|window| window.id()),
        );
    }
    windows
}

fn review_files(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let review = review_here(ctx)?;
    let buffer = ctx.editor.window().buffer();
    if ctx.editor.review_files(review) == Some(buffer) {
        // From the pane: it closes, and the review is selected again.
        ctx.editor.hide_review_files(review);
        if let Some(window) = review_windows(ctx.editor, review).first() {
            ctx.editor.select_window(*window);
        }
        ctx.request_redraw();
        return Ok(());
    }
    let window = ctx.editor.window().id();
    let pane = (ctx.editor.show_review_files(review, window)).map_err(|error| match error {
        NoPair::TooSmall => CommandError::new("No room for the files pane"),
        NoPair::NoBuffer | NoPair::Paired => CommandError::new("Not in a review"),
    })?;
    ctx.editor.select_window(pane);
    ctx.request_redraw();
    Ok(())
}

/// Moves the files pane's cursor `count` files on, back for a negative count, as far as it
/// goes.
fn walk(ctx: &mut Ctx<'_>, count: isize) -> Result<(), CommandError> {
    let review = current_pane(ctx)?;
    // A clone of the rows is O(1): they are shared.
    let Some(rows) = ctx.editor.review_files_rows(review).cloned() else {
        return Err(CommandError::new("Not in a review's files pane"));
    };
    let edge = if count >= 0 {
        "No next file"
    } else {
        "No file before"
    };
    super::seek_in(ctx, &rows, Seek::File, count, edge)
}

fn review_files_next(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    walk(ctx, args.count())
}

fn review_files_previous(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    walk(ctx, args.count().saturating_neg())
}

/// Puts the header of the file at the pane's cursor at the top of each window showing the
/// review, and selects the first if `select`.
fn show_file(ctx: &mut Ctx<'_>, select: bool) -> Result<(), CommandError> {
    let review = current_pane(ctx)?;
    // No place leaves, or enters, a buffer that may hold secrets (ADR 0044).
    if ctx.editor.current().1.is_secret() {
        return Err(CommandError::new("The files pane may hold secrets now"));
    }
    let line = {
        let (window, buffer) = ctx.editor.current();
        buffer
            .text()
            .offset_to_point(window.selections.primary().cursor)
            .row
    };
    let path: Box<[u8]> = (ctx.editor.review_files_rows(review))
        .and_then(|rows| rows.at(line))
        .filter(|row| row.kind == RowKind::File)
        .map(|row| row.path.into())
        .ok_or_else(|| CommandError::new("No file here"))?;
    let mut shown = None;
    for window in review_windows(ctx.editor, review) {
        let Some(buffer) = ctx.editor.window_by_id(window).map(|shown| shown.buffer()) else {
            continue;
        };
        if ctx
            .editor
            .buffer(buffer)
            .is_none_or(|shown| shown.is_secret())
        {
            continue;
        }
        let header = (ctx.editor.review_rows_of(buffer))
            .and_then(|rows| rows.line_of(&path, None))
            .zip(ctx.editor.buffer(buffer))
            .map(|(line, buffer)| buffer.text().line_start(line));
        let Some(offset) = header else {
            continue;
        };
        let scroll = Scroll {
            start: offset,
            left: 0,
        };
        ctx.editor
            .restore_positions_in(window, SelectionSet::new(offset), scroll);
        shown.get_or_insert(window);
    }
    let Some(first) = shown else {
        return Err(CommandError::new("The review does not show that file now"));
    };
    if select {
        ctx.editor.select_window(first);
    }
    ctx.request_redraw();
    Ok(())
}

fn review_files_visit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    show_file(ctx, true)
}

fn review_files_display(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    show_file(ctx, false)
}

fn review_filter(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let review = review_here(ctx)?;
    // A filter is quoted in the review's title and saved with it (ADR 0044).
    if ctx.answer_secret() {
        return Err(CommandError::new(
            "A filter holding what may be secret is not kept",
        ));
    }
    let filter = Filter::new(ctx.answer().unwrap_or_default());
    let said = match &filter {
        Some(filter) => format!(
            "Showing the files \"{}\" matches",
            super::one_line(filter.text(), 40)
        ),
        None => "Showing every file".to_owned(),
    };
    ctx.editor.set_review_filter(review, filter);
    ctx.editor.message(said);
    ctx.request_redraw();
    Ok(())
}

fn review_files_keys(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let text = super::keys_text(
        "Keys in a review's files pane",
        FILES_KEYS,
        "In the review, s selects the pane and / filters its files.",
    );
    super::show_keys(ctx, KEYS_BUFFER, &text)
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
