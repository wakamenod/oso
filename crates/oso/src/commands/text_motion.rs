//! Commands that move by paragraphs, sentences and pages, mark them and kill sentences, as
//! Emacs's do (ADR 0190).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and does not know its keys.
//!
//! - Each works at every cursor of the selected window (ADR 0007), within a budget they share
//!   (`WalkBudget::for_cursors`); a step past it leaves that cursor where the step before left
//!   it, and the command signals "Count too large".
//! - The motions keep a shown region and grow it, and select with Shift (ADR 0071). At the
//!   buffer's edge they stop with no signal, as Emacs's do.
//! - `mark-paragraph` and `mark-page` set every mark, put the old one in the ring and show the
//!   region; repeated, or while a region shows, they move the marks on instead.
//! - In Markdown, headings and fence lines are paragraphs of their own.

use std::ops::Range;

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::display_map::DisplaySnapshot;
use oso_core::motion::{self, WalkBudget};
use oso_core::selection::{MarkActive, Selection};

use super::budget_left;
use super::kill::{direction_of, kill};
use super::motion::move_cursors;

/// The commands of paragraphs, sentences and pages.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "forward-paragraph",
        doc: "Move point to the end of the paragraph, N times (backward if N is negative).\n\nA paragraph ends at the start of a line of only spaces, tabs and form feeds, or of one that starts with a form feed. In Markdown, a heading or a fence line is a paragraph of its own. At the end of the buffer, stay there.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: forward_paragraph,
        state: None,
    },
    Command {
        name: "backward-paragraph",
        doc: "Move point to the start of the paragraph, N times (forward if N is negative).\n\nThe start is the separating line just above the paragraph, or its first line where there is none. See `forward-paragraph`.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: backward_paragraph,
        state: None,
    },
    Command {
        name: "mark-paragraph",
        doc: "Put point at the start of the paragraph and the mark at its end, and show the region.\n\nWith N, mark N paragraphs from here (backward if N is negative). Repeated, or while the region shows, move the mark N paragraphs on instead. The old mark goes first in the mark ring. Paragraphs are as in `forward-paragraph`.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: mark_paragraph,
        state: None,
    },
    Command {
        name: "forward-sentence",
        doc: "Move point to the end of the sentence, N times (backward if N is negative).\n\nA sentence ends at `.`, `?`, `!` or `…` and any closing quotes and brackets after it, followed by a space or the end of a line; at `。`, `！`, `？` or `．`, with no space after; and at the end of its paragraph.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: forward_sentence,
        state: None,
    },
    Command {
        name: "backward-sentence",
        doc: "Move point to the start of the sentence, N times (forward if N is negative).\n\nSee `forward-sentence`.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: backward_sentence,
        state: None,
    },
    Command {
        name: "kill-sentence",
        doc: "Kill from every cursor to the end of the sentence, N times (backward if N is negative).\n\nSentences are as in `forward-sentence`. A kill right after a kill appends to it.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: kill_sentence,
        state: None,
    },
    Command {
        name: "backward-kill-sentence",
        doc: "Kill from every cursor back to the start of the sentence, N times (forward if N is negative).\n\nSee `kill-sentence`. Appended to a kill before it, the text goes first.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: backward_kill_sentence,
        state: None,
    },
    Command {
        name: "forward-page",
        doc: "Move point to the start of the next page, N times (backward if N is negative).\n\nA page ends at a form feed at the start of a line; the next one starts just after it. At the end of the buffer, stay there.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: forward_page,
        state: None,
    },
    Command {
        name: "backward-page",
        doc: "Move point to the start of the page, N times (forward if N is negative).\n\nSee `forward-page`.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: backward_page,
        state: None,
    },
    Command {
        name: "mark-page",
        doc: "Put point at the start of the page and the mark at its end, and show the region.\n\nWith N, mark the page N pages from this one (before it if N is negative). Repeated, or while the region shows, move the mark N pages on instead, one with no N. The old mark goes first in the mark ring. Pages are as in `forward-page`.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: mark_page,
        state: None,
    },
];

/// Whether the selected window's buffer reads headings and fences as paragraphs of their own.
fn markdown(ctx: &Ctx<'_>) -> bool {
    matches!(ctx.editor.current().1.mode(), Mode::Markdown | Mode::Plan)
}

/// A budget shared by the selected window's cursors.
fn budget(ctx: &Ctx<'_>) -> WalkBudget {
    WalkBudget::for_cursors(ctx.editor.window().selections.len())
}

/// Moves every cursor `n` times with `motion`, which takes the snapshot, a selection, the
/// count and the budget. The buffer's edge stops it with no signal, as in Emacs.
fn move_by(
    ctx: &mut Ctx<'_>,
    n: isize,
    mut motion: impl FnMut(
        &DisplaySnapshot,
        &mut Selection,
        isize,
        &mut WalkBudget,
    ) -> Result<(), motion::Edge>,
) -> Result<(), CommandError> {
    let mut budget = budget(ctx);
    move_cursors(ctx, |display, selection| {
        let _ = motion(display, selection, n, &mut budget);
        Ok(())
    })?;
    budget_left(budget)
}

fn paragraphs(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    let markdown = markdown(ctx);
    move_by(ctx, n, |display, selection, n, budget| {
        motion::forward_paragraph(display, markdown, selection, n, budget)
    })
}

fn forward_paragraph(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    paragraphs(ctx, args.count())
}

fn backward_paragraph(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    paragraphs(ctx, args.count().saturating_neg())
}

fn forward_sentence(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, args.count(), motion::forward_sentence)
}

fn backward_sentence(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, args.count().saturating_neg(), motion::forward_sentence)
}

fn forward_page(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, args.count(), motion::forward_page)
}

fn backward_page(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, args.count().saturating_neg(), motion::forward_page)
}

/// Where `n` sentences from `offset` go, or `offset` if the budget ran out on the way.
fn sentences_from(
    display: &DisplaySnapshot,
    offset: usize,
    n: isize,
    budget: &mut WalkBudget,
) -> usize {
    let mut moved = Selection::at(offset);
    let _ = motion::forward_sentence(display, &mut moved, n, budget);
    if budget.is_spent() {
        offset
    } else {
        moved.cursor
    }
}

/// Kills from every cursor across `n` sentences, backward if `n` is negative. A cursor the
/// budget runs out on kills nothing, and the command signals after the others have killed.
fn kill_sentences(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    let mut budget = budget(ctx);
    let (window, buffer) = ctx.editor.current();
    let display = buffer.display();
    let ranges: Vec<Range<usize>> = window
        .selections
        .iter()
        .map(|selection| {
            let to = sentences_from(&display, selection.cursor, n, &mut budget);
            selection.cursor.min(to)..selection.cursor.max(to)
        })
        .collect();
    kill(ctx, ranges, direction_of(n))?;
    budget_left(budget)
}

fn kill_sentence(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    kill_sentences(ctx, args.count())
}

fn backward_kill_sentence(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    kill_sentences(ctx, args.count().saturating_neg())
}

/// What a mark command does (ADR 0190). Repeated, or while the region shows, `place` moves
/// every mark on, told so; else the primary's old mark goes first in the ring, and `place`
/// puts every cursor and mark around its object. The region shows after either.
fn mark_objects(ctx: &mut Ctx<'_>, mut place: impl FnMut(&DisplaySnapshot, &mut Selection, bool)) {
    let repeated = ctx.repeats() > 0;
    let (window, buffer) = ctx.editor.current_mut();
    let display = buffer.display();
    let selections = &mut window.selections;
    let extend = repeated || selections.mark_active().is_active();
    if !extend {
        selections.push_marks();
    }
    // Shown first, so that regions that come to overlap merge.
    selections.set_mark_active(MarkActive::Active);
    selections.update(|selection| {
        place(&display, selection, extend);
        Ok::<(), ()>(())
    });
    ctx.request_redraw();
}

/// Puts `selection`'s cursor at the start of `object` and its mark at the end.
fn select(selection: &mut Selection, object: Range<usize>) {
    selection.mark = Some(object.end);
    selection.move_to(object.start);
}

/// Where `n` objects from `offset` go by `motion`, which moves a selection.
fn moved(offset: usize, motion: impl FnOnce(&mut Selection) -> Result<(), motion::Edge>) -> usize {
    let mut selection = Selection::at(offset);
    let _ = motion(&mut selection);
    selection.cursor
}

fn mark_paragraph(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let n = args.count();
    if n == 0 {
        return Err(CommandError::new("Cannot mark zero paragraphs"));
    }
    let markdown = markdown(ctx);
    let mut budget = budget(ctx);
    let paragraphs = |display: &DisplaySnapshot, offset, n, budget: &mut WalkBudget| {
        moved(offset, |s| {
            motion::forward_paragraph(display, markdown, s, n, budget)
        })
    };
    mark_objects(ctx, |display, selection, extend| {
        if extend {
            let mark = selection.mark.unwrap_or(selection.cursor);
            selection.mark = Some(paragraphs(display, mark, n, &mut budget));
        } else if n == 1 {
            let paragraph =
                motion::paragraph_bounds(display, selection.cursor, markdown, &mut budget);
            select(selection, paragraph);
        } else {
            // As Emacs: N paragraphs on, and back as many from there.
            let end = paragraphs(display, selection.cursor, n, &mut budget);
            let start = paragraphs(display, end, n.saturating_neg(), &mut budget);
            select(selection, start.min(end)..start.max(end));
        }
    });
    budget_left(budget)
}

fn mark_page(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let mut budget = budget(ctx);
    let pages = |display: &DisplaySnapshot, offset, n, budget: &mut WalkBudget| {
        moved(offset, |s| motion::forward_page(display, s, n, budget))
    };
    // As Emacs: first to the page N pages away, with no N this one.
    let away = match args.prefix() {
        Some(n) if n < 0 => n.saturating_sub(1),
        Some(n) => n,
        None => 0,
    };
    mark_objects(ctx, |display, selection, extend| {
        if extend {
            let mark = selection.mark.unwrap_or(selection.cursor);
            selection.mark = Some(pages(display, mark, args.count(), &mut budget));
        } else {
            let page = pages(display, selection.cursor, away, &mut budget);
            let end = pages(display, page, 1, &mut budget);
            let start = pages(display, end, -1, &mut budget);
            select(selection, start..end);
        }
    });
    budget_left(budget)
}

#[cfg(test)]
mod tests;
