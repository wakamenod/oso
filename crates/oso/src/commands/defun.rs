//! Commands that move by defuns, mark one and show one (ADR 0190, step 3).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! A defun is found by the line it starts on, in each mode's way (`oso_core::sexp`). The
//! motions move every cursor, select with Shift, and set the mark where the cursors were when
//! no region shows and they do not repeat. `mark-defun` and `reposition-window` leave one
//! cursor, as Emacs's do.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::motion::{Edge, WalkBudget};
use oso_core::selection::{MarkActive, Selection};
use oso_core::sexp::{self, Syntax, Table};
use oso_core::window::Recenter;

use super::budget_left;
use super::mark::shift_select;

/// The defun commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "beginning-of-defun",
        doc: "Move point back to the beginning of a defun, N times (forward to the following ones if N is negative).\n\nA defun starts on a line that begins one in the buffer's mode, at any indentation: an item such as `fn` or `impl` in Rust, a function or a class in JavaScript, a rule in CSS, a heading in Markdown, a table in TOML, a section in LaTeX; in Fundamental, `(` or `{` at the line's start. Point goes to its first non-blank character. Where there are no more, go to the start of the buffer.\n\nThe mark is set where point was, unless the region shows or the command repeats.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: beginning_of_defun,
        state: None,
    },
    Command {
        name: "end-of-defun",
        doc: "Move point to the end of the defun it is in, or of the next one, N times (back to the ends of earlier ones if N is negative).\n\nThe end is the start of the line after the defun: after the braces of a function, at the `;` of a declaration, before the next heading of the same level or above in Markdown. Where there are no more, go to the end of the buffer.\n\nThe mark is set where point was, unless the region shows or the command repeats.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: end_of_defun,
        state: None,
    },
    Command {
        name: "mark-defun",
        doc: "Put point at the beginning of this defun and the mark at its end, and show the region.\n\nThe defun is the one point is in or the one after it, with the comment just above it. With N, mark N defuns from there (back if N is negative). Repeated, or while the region shows, move the mark N more defuns on. One cursor is left; the old mark goes first in the mark ring.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: mark_defun,
        state: None,
    },
    Command {
        name: "reposition-window",
        doc: "Scroll so that the defun point is in starts at the top of the window, with the comment just above it.\n\nIf point would then be below the window, put point's line at the bottom instead. One cursor is left.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: reposition_window,
        state: None,
    },
];

/// The signal of a command that finds no defun.
const NO_DEFUN: &str = "No defun here";

type Motion = fn(&Syntax<'_>, &mut Selection, isize, &mut WalkBudget) -> Result<(), Edge>;

/// Moves every cursor `n` times with `motion`, after what Shift selects and, unless the
/// region shows or the command repeats, the mark where they were.
fn move_by_defuns(ctx: &mut Ctx<'_>, n: isize, motion: Motion) -> Result<(), CommandError> {
    shift_select(ctx);
    let repeated = ctx.repeats() > 0;
    let selections = &mut ctx.editor.current_mut().0.selections;
    if !repeated && !selections.mark_active().is_active() {
        selections.push_marks();
        ctx.editor.message("Mark set");
    }
    let (window, buffer) = ctx.editor.current_mut();
    let display = buffer.display();
    let syntax = Syntax::new(&display, buffer.highlights(), Table::of(buffer.mode()));
    let mut budget = WalkBudget::for_cursors(window.selections.len());
    // The buffer's edge stops it with no signal, as in Emacs and as paragraphs do.
    let update = window.selections.update(|selection| {
        let _ = motion(&syntax, selection, n, &mut budget);
        Ok::<(), ()>(())
    });
    if update.changed {
        ctx.request_redraw();
    }
    budget_left(budget)
}

fn beginning_of_defun(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by_defuns(ctx, args.count(), sexp::beginning_of_defun)
}

fn end_of_defun(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by_defuns(ctx, args.count(), sexp::end_of_defun)
}

fn mark_defun(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let n = match args.count() {
        0 => 1,
        n => n,
    };
    let repeated = ctx.repeats() > 0;
    let (window, buffer) = ctx.editor.current_mut();
    let display = buffer.display();
    let syntax = Syntax::new(&display, buffer.highlights(), Table::of(buffer.mode()));
    let selections = &mut window.selections;
    selections.keep_primary();
    let primary = *selections.primary();
    let mut budget = WalkBudget::new();
    let shows = selections.mark_active().is_active();
    let (start, end) = match primary.mark {
        Some(mark) if repeated || shows => {
            // The mark goes N more defuns on, and point stays.
            let mut on = Selection::at(mark);
            // At the buffer's edge it stays, with no signal.
            let _ = sexp::end_of_defun(&syntax, &mut on, n, &mut budget);
            (primary.cursor, on.cursor)
        }
        _ => {
            let Some(bounds) = sexp::defun_bounds(&syntax, primary.cursor, &mut budget) else {
                budget_left(budget)?;
                return Err(CommandError::new(NO_DEFUN));
            };
            if n > 0 {
                let mut end = Selection::at(bounds.end);
                // Past the last there is nothing more to take in.
                let _ = sexp::end_of_defun(&syntax, &mut end, n - 1, &mut budget);
                (bounds.start, end.cursor)
            } else {
                let mut start = Selection::at(bounds.start);
                let _ = sexp::beginning_of_defun(&syntax, &mut start, -1 - n, &mut budget);
                let start = sexp::defun_bounds(&syntax, start.cursor, &mut budget)
                    .map_or(start.cursor, |earlier| earlier.start.min(start.cursor));
                (start, bounds.end)
            }
        }
    };
    budget_left(budget)?;
    if !(repeated || shows) {
        selections.push_marks();
    }
    selections.update(|selection| {
        selection.mark = Some(end);
        selection.move_to(start);
        Ok::<(), ()>(())
    });
    selections.set_mark_active(MarkActive::Active);
    ctx.request_redraw();
    Ok(())
}

fn reposition_window(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current_mut();
    window.selections.keep_primary();
    let display = buffer.display();
    let syntax = Syntax::new(&display, buffer.highlights(), Table::of(buffer.mode()));
    let cursor = window.selections.primary().cursor;
    let mut budget = WalkBudget::new();
    // The defun point is in, or the one before it: not one below point.
    let start = match sexp::defun_bounds(&syntax, cursor, &mut budget) {
        Some(bounds) if bounds.start <= cursor => Some(bounds.start),
        _ => {
            let mut before = Selection::at(cursor);
            match sexp::beginning_of_defun(&syntax, &mut before, 1, &mut budget) {
                // Past the first defun it goes to the buffer's start, where the one found
                // may be below point.
                Ok(()) if before.cursor < cursor => {
                    sexp::defun_bounds(&syntax, before.cursor, &mut budget)
                        .filter(|bounds| bounds.start <= cursor)
                        .map(|bounds| bounds.start.min(before.cursor))
                }
                _ => None,
            }
        }
    };
    budget_left(budget)?;
    let Some(start) = start else {
        return Err(CommandError::new(NO_DEFUN));
    };
    let row_of = |offset| match window.wrap_of(&display) {
        Some(wrap) => wrap.row_of(&display, offset),
        None => display.offset_to_display_point(offset).row,
    };
    let below = row_of(cursor).saturating_sub(row_of(start));
    let target = if below < window.size().rows {
        Recenter::Line(isize::try_from(below).unwrap_or(isize::MAX))
    } else {
        Recenter::Line(-1)
    };
    if ctx.editor.recenter(target) {
        ctx.request_redraw();
    }
    Ok(())
}
