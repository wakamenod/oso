//! Commands for the mark and the region: setting the mark, exchanging it with the
//! cursor, going back through the mark ring and marking the whole buffer (ADR 0071).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! Every cursor of the selected window has its own mark, and the window shows all
//! their regions or none (ADR 0007, `transient-mark-mode`). An edit hides them, in
//! `oso-core`, and so do `C-g` and `C-SPC C-SPC`. A motion with Shift shows them
//! from where it started, as Emacs's `shift-select-mode` (`shift_select`).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::motion;
use oso_core::selection::MarkActive;

/// The mark commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "set-mark-command",
        doc: "Set the mark at every cursor, and show the region.\n\nThe old mark goes first in the mark ring. Repeated at once, hide the region and keep the mark, to come back to with `C-u C-SPC`. With a prefix argument, go to the mark instead, as `pop-to-mark-command` does.",
        args: ArgSpec::Count,
        run: set_mark_command,
    },
    Command {
        name: "exchange-point-and-mark",
        doc: "Put every cursor where its mark is and the mark where it was, and show the region.\n\nWith a prefix argument, hide the region instead. Signal \"No mark set in this buffer\" if the primary cursor has no mark.",
        args: ArgSpec::Count,
        run: exchange_point_and_mark,
    },
    Command {
        name: "pop-to-mark-command",
        doc: "Go to the mark with one cursor, and hide the region.\n\nThe mark then goes back to the newest in the mark ring, and the old one goes last there, so repeating it goes back through the ring. Signal \"No mark set in this buffer\" if the primary cursor has no mark.",
        args: ArgSpec::None,
        run: pop_to_mark_command,
    },
    Command {
        name: "mark-whole-buffer",
        doc: "Put one cursor at the start of the buffer and its mark at the end, and show the region.\n\nWhere the cursor was goes first in the mark ring, after the old mark.",
        args: ArgSpec::None,
        run: mark_whole_buffer,
    },
];

/// The signal for a command that needs the primary cursor's mark.
const NO_MARK: &str = "No mark set in this buffer";

fn set_mark_command(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    if args.raw().is_some() {
        return pop_to_mark_command(ctx, args);
    }
    let repeated = ctx.repeats() > 0;
    let selections = &mut ctx.editor.current_mut().0.selections;
    if repeated && selections.mark_active().is_active() {
        selections.set_mark_active(MarkActive::Inactive);
        ctx.request_redraw();
        ctx.editor.message("Mark deactivated");
        return Ok(());
    }
    selections.push_marks();
    selections.set_mark_active(MarkActive::Active);
    ctx.request_redraw();
    ctx.editor.message("Mark set");
    Ok(())
}

fn exchange_point_and_mark(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let selections = &mut ctx.editor.current_mut().0.selections;
    if selections.primary().mark.is_none() {
        return Err(CommandError::new(NO_MARK));
    }
    let region = match args.raw() {
        Some(_) => MarkActive::Inactive,
        None => MarkActive::Active,
    };
    let before = selections.set_mark_active(region);
    let update = selections.update(|selection| {
        if let Some(mark) = selection.mark.replace(selection.cursor) {
            selection.move_to(mark);
        }
        Ok::<(), ()>(())
    });
    if update.changed || before.is_active() != region.is_active() {
        ctx.request_redraw();
    }
    Ok(())
}

fn pop_to_mark_command(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let selections = &mut ctx.editor.current_mut().0.selections;
    let before = (
        selections.primary().cursor,
        selections.len(),
        selections.mark_active(),
    );
    if !selections.pop_mark() {
        return Err(CommandError::new(NO_MARK));
    }
    let after = (
        selections.primary().cursor,
        selections.len(),
        selections.mark_active(),
    );
    if after != before {
        ctx.request_redraw();
    }
    if after.0 == before.0 {
        ctx.editor.message("Mark popped");
    }
    Ok(())
}

fn mark_whole_buffer(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current_mut();
    let end = motion::buffer_end(&buffer.display());
    let selections = &mut window.selections;
    selections.keep_primary();
    // As Emacs's two `push-mark`s: the old mark, then the cursor, go to the ring.
    selections.push_marks();
    selections.push_marks();
    selections.update(|selection| {
        selection.mark = Some(end);
        selection.move_to(0);
        Ok::<(), ()>(())
    });
    selections.set_mark_active(MarkActive::Active);
    ctx.request_redraw();
    ctx.editor.message("Mark set");
    Ok(())
}

/// What a motion does with the region first, as Emacs's `handle-shift-selection`.
/// A motion with Shift shows the regions from every cursor, unless they already
/// show; one without it hides those that a Shift showed. Returns whether that
/// changed what shows. A command calls it before it moves the cursors.
pub(super) fn shift_select(ctx: &mut Ctx<'_>) -> bool {
    let shifted = ctx.shifted();
    let selections = &mut ctx.editor.current_mut().0.selections;
    match (shifted, selections.mark_active()) {
        (true, MarkActive::Inactive) => {
            selections.push_marks();
            selections.set_mark_active(MarkActive::Shifted);
            false
        }
        (false, MarkActive::Shifted) => {
            selections.set_mark_active(MarkActive::Inactive);
            true
        }
        _ => false,
    }
}
