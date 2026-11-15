//! A listing's marks (ADR 0134, step 1b): `m` marks the row at point and moves down, `u`
//! unmarks it and moves down, `DEL` moves up and unmarks, `U` unmarks every row and `t` toggles
//! every mark. With a count, that many rows; with the region shown, the rows it covers.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command says which lines it means, and
//! the host, which keeps the names marked, changes them. Marks open, show and write nothing, so
//! these are `Look`.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::ListingRequest;

use super::{listing, move_rows};

/// The mark commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "dired-mark",
        doc: "In a listing, mark the row at point and move down, or the next rows with a count, or the rows the region covers.\n\nA mark is a `*` before the row; `.` and `..` are never marked. Marks stay over a new listing of the directory.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: mark,
        state: None,
    },
    Command {
        name: "dired-unmark",
        doc: "In a listing, unmark the row at point and move down, or the next rows with a count, or the rows the region covers.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: unmark,
        state: None,
    },
    Command {
        name: "dired-unmark-backward",
        doc: "In a listing, move up a row and unmark it, or that many rows with a count.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: unmark_backward,
        state: None,
    },
    Command {
        name: "dired-unmark-all-marks",
        doc: "In a listing, unmark every row.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: unmark_all,
        state: None,
    },
    Command {
        name: "dired-toggle-marks",
        doc: "In a listing, mark every row that is not marked and unmark those that are.\n\n`.` and `..` are never marked.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: toggle_marks,
        state: None,
    },
];

/// The listing's keys for marks.
pub const KEYS: &[(&str, &str)] = &[
    ("m", "dired-mark"),
    ("u", "dired-unmark"),
    ("DEL", "dired-unmark-backward"),
    ("U", "dired-unmark-all-marks"),
    ("t", "dired-toggle-marks"),
];

/// The lines a mark command means: those the region covers while it is shown, else `count`
/// rows from each cursor's, down, or up for a negative count. Whether the region gave them.
fn marked_lines(ctx: &Ctx<'_>, count: isize) -> (Vec<usize>, bool) {
    let (window, buffer) = ctx.editor.current();
    let text = buffer.text();
    let mut lines = Vec::new();
    let region = window.selections.mark_active().is_active();
    for selection in window.selections.iter() {
        let at = text.offset_to_point(selection.cursor).row;
        match selection.mark.filter(|_| region) {
            Some(mark) => {
                let (start, end) = (selection.cursor.min(mark), selection.cursor.max(mark));
                let first = text.offset_to_point(start).row;
                let last_point = text.offset_to_point(end);
                // A region that ends where a line starts does not cover that line.
                let last = if last_point.column == 0 && end > start {
                    last_point.row.saturating_sub(1)
                } else {
                    last_point.row
                };
                lines.extend(first..=last);
            }
            None => {
                let steps = count.unsigned_abs();
                lines.extend((0..steps).map(|step| {
                    if count < 0 {
                        at.saturating_sub(step)
                    } else {
                        at + step
                    }
                }));
            }
        }
    }
    lines.sort_unstable();
    lines.dedup();
    (lines, region)
}

/// Marks or unmarks the lines meant, then moves past them unless the region gave them.
fn set_marks(ctx: &mut Ctx<'_>, count: isize, on: bool) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    let (lines, region) = marked_lines(ctx, count);
    ctx.editor
        .request_listing(ListingRequest::Mark { buffer, lines, on });
    if !region {
        move_rows(ctx, count)?;
    }
    ctx.request_redraw();
    Ok(())
}

fn mark(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    set_marks(ctx, args.count(), true)
}

fn unmark(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    set_marks(ctx, args.count(), false)
}

fn unmark_backward(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    let count = args.count();
    move_rows(ctx, count.saturating_neg())?;
    let (lines, _) = marked_lines(ctx, 1);
    let lines = lines
        .into_iter()
        .flat_map(|line| (0..count.unsigned_abs()).map(move |step| line + step))
        .collect();
    ctx.editor.request_listing(ListingRequest::Mark {
        buffer,
        lines,
        on: false,
    });
    ctx.request_redraw();
    Ok(())
}

fn unmark_all(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    ctx.editor
        .request_listing(ListingRequest::UnmarkAll { buffer });
    Ok(())
}

fn toggle_marks(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    ctx.editor
        .request_listing(ListingRequest::ToggleMarks { buffer });
    Ok(())
}
