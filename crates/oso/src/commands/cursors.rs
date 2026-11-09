//! Commands that add cursors, as multiple-cursors.el's (ADR 0091): at the next or previous
//! occurrence of the region or the word, skipping one, at every one, and on each line of
//! the region.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! What they look for, where, and the worker that searches past a key's budget are in
//! `oso-core`. They move and mark and change no text, so they are `Look`; the agent
//! channel already runs nothing while a buffer that may hold secrets is shown
//! (ADR 0085).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{CursorError, Toward};

/// The commands that add cursors.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "mark-next-like-this",
        doc: "Add a cursor at the next occurrence of the region, or of the word at point.\n\nWith no region, the word at each cursor is selected first, and only whole words match. The new cursor's region covers the match, and it becomes the primary. With a count, add that many; with a negative one, remove that many cursors from the end; with 0, skip, as `skip-to-next-like-this`.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: mark_next_like_this,
    },
    Command {
        name: "mark-previous-like-this",
        doc: "Add a cursor at the previous occurrence of the region, or of the word at point.\n\nAs `mark-next-like-this`, going back from the first cursor. A negative count removes cursors from the start.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: mark_previous_like_this,
    },
    Command {
        name: "skip-to-next-like-this",
        doc: "Move the last cursor to the next occurrence after it.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: skip_to_next_like_this,
    },
    Command {
        name: "skip-to-previous-like-this",
        doc: "Move the first cursor to the previous occurrence before it.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: skip_to_previous_like_this,
    },
    Command {
        name: "mark-all-like-this",
        doc: "Put a cursor on every occurrence of the region, or of the word at point, in the buffer.\n\nWith more than 1000, add none and say how many there are.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: mark_all_like_this,
    },
    Command {
        name: "edit-lines",
        doc: "Put a cursor on each line from the mark's to the cursor's, at the cursor's column.\n\nA shorter line gets one at its end. The region hides.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: edit_lines,
    },
];

fn mark_next_like_this(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let done = ctx.editor.mark_like_this(Toward::Next, args.count());
    added(ctx, done)
}

fn mark_previous_like_this(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let done = ctx.editor.mark_like_this(Toward::Previous, args.count());
    added(ctx, done)
}

fn skip_to_next_like_this(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let done = ctx.editor.skip_like_this(Toward::Next);
    added(ctx, done)
}

fn skip_to_previous_like_this(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let done = ctx.editor.skip_like_this(Toward::Previous);
    added(ctx, done)
}

fn mark_all_like_this(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let done = ctx.editor.mark_all_like_this();
    added(ctx, done)
}

fn edit_lines(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let done = ctx.editor.edit_lines();
    added(ctx, done)
}

/// Draws what a key that adds cursors changed, which it may have done before it failed,
/// as selecting the words, and signals why it added none or no more.
fn added(ctx: &mut Ctx<'_>, done: Result<(), CursorError>) -> Result<(), CommandError> {
    ctx.request_redraw();
    done.map_err(|error| CommandError::new(error.to_string()))
}
