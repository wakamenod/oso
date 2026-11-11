//! Commands that split, delete, select and resize the tiled windows (ADR 0112). They change
//! no text or file, and each refuses inside a prompt, whose window is not in the tree.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::frame::{Side, Way};

use super::not_in_prompt;

/// The window commands. All are `Look`: none edits text or a file, and the person can undo
/// any of them with keys (ADR 0112).
pub const COMMANDS: &[Command] = &[
    Command {
        name: "split-window-below",
        doc: "Split the selected window into two windows, one above the other.\n\nThe selected window stays selected, above; the new one, below, shows the same buffer from the same place, with the primary cursor and its mark.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: split_window_below,
    },
    Command {
        name: "split-window-right",
        doc: "Split the selected window into two side-by-side windows.\n\nThe selected window stays selected, at the left; the new one, at the right, shows the same buffer from the same place, with the primary cursor and its mark.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: split_window_right,
    },
    Command {
        name: "split-window-below-and-select",
        doc: "Split the selected window into two windows, one above the other, and select the new one below.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: split_window_below_and_select,
    },
    Command {
        name: "split-window-right-and-select",
        doc: "Split the selected window into two side-by-side windows, and select the new one at the right.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: split_window_right_and_select,
    },
    Command {
        name: "delete-window",
        doc: "Delete the selected window.\n\nIts space goes to a neighbour, and the window selected before it is selected again. The buffer it showed stays.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: delete_window,
    },
    Command {
        name: "delete-other-windows",
        doc: "Make the selected window fill the frame, deleting every other window.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: delete_other_windows,
    },
    Command {
        name: "other-window",
        doc: "Select another window, N windows on in cyclic order.\n\nWindows are ordered left to right, top to bottom. A negative N goes back.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: other_window,
    },
    Command {
        name: "balance-windows",
        doc: "Make the windows side by side the same width, and those stacked the same height.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: balance_windows,
    },
    Command {
        name: "enlarge-window",
        doc: "Make the selected window N rows taller.\n\nThe rows come from the windows below it, or above it. A negative N makes it shorter.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: enlarge_window,
    },
    Command {
        name: "shrink-window",
        doc: "Make the selected window N rows shorter.\n\nThe rows go to the window below it, or above it. A negative N makes it taller.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: shrink_window,
    },
    Command {
        name: "enlarge-window-horizontally",
        doc: "Make the selected window N columns wider.\n\nThe columns come from the windows at its right, or at its left. A negative N makes it narrower.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: enlarge_window_horizontally,
    },
    Command {
        name: "shrink-window-horizontally",
        doc: "Make the selected window N columns narrower.\n\nThe columns go to the window at its right, or at its left. A negative N makes it wider.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: shrink_window_horizontally,
    },
    Command {
        name: "windmove-left",
        doc: "Select the window to the left of the selected one, beside the cursor's row.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: windmove_left,
    },
    Command {
        name: "windmove-right",
        doc: "Select the window to the right of the selected one, beside the cursor's row.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: windmove_right,
    },
    Command {
        name: "windmove-up",
        doc: "Select the window above the selected one, over the cursor's column.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: windmove_up,
    },
    Command {
        name: "windmove-down",
        doc: "Select the window below the selected one, under the cursor's column.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: windmove_down,
    },
];

fn split(ctx: &mut Ctx<'_>, way: Way, select: bool) -> Result<(), CommandError> {
    not_in_prompt(ctx)?;
    ctx.editor
        .split_window(way, select)
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

fn split_window_below(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    split(ctx, Way::Down, false)
}

fn split_window_right(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    split(ctx, Way::Across, false)
}

fn split_window_below_and_select(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    split(ctx, Way::Down, true)
}

fn split_window_right_and_select(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    split(ctx, Way::Across, true)
}

fn delete_window(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    not_in_prompt(ctx)?;
    ctx.editor
        .delete_window()
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

fn delete_other_windows(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    not_in_prompt(ctx)?;
    if ctx.editor.delete_other_windows() {
        ctx.request_redraw();
    }
    Ok(())
}

/// Runs `select`, and draws a frame if the selected window changed.
fn selecting(
    ctx: &mut Ctx<'_>,
    select: impl FnOnce(&mut Ctx<'_>) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    not_in_prompt(ctx)?;
    let before = ctx.editor.window().id();
    select(ctx)?;
    if ctx.editor.window().id() != before {
        ctx.request_redraw();
    }
    Ok(())
}

fn other_window(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    selecting(ctx, |ctx| {
        ctx.editor.other_window(args.count());
        Ok(())
    })
}

fn balance_windows(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    not_in_prompt(ctx)?;
    if ctx.editor.balance_windows() {
        ctx.request_redraw();
    }
    Ok(())
}

/// Resizes the selected window by `cells` along `way`.
fn resize(ctx: &mut Ctx<'_>, way: Way, cells: isize) -> Result<(), CommandError> {
    not_in_prompt(ctx)?;
    if cells == 0 {
        return Ok(());
    }
    ctx.editor
        .resize_window(way, cells)
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

fn enlarge_window(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    resize(ctx, Way::Down, args.count())
}

fn shrink_window(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    resize(ctx, Way::Down, args.count().saturating_neg())
}

fn enlarge_window_horizontally(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    resize(ctx, Way::Across, args.count())
}

fn shrink_window_horizontally(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    resize(ctx, Way::Across, args.count().saturating_neg())
}

fn windmove(ctx: &mut Ctx<'_>, side: Side) -> Result<(), CommandError> {
    selecting(ctx, |ctx| {
        ctx.editor
            .windmove(side)
            .map_err(|error| CommandError::new(error.to_string()))
    })
}

fn windmove_left(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    windmove(ctx, Side::Left)
}

fn windmove_right(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    windmove(ctx, Side::Right)
}

fn windmove_up(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    windmove(ctx, Side::Up)
}

fn windmove_down(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    windmove(ctx, Side::Down)
}

#[cfg(test)]
mod tests;
