//! The indentation commands of ADR 0115 step 2: `TAB` reindenting by the buffer's mode, the
//! line breaks that indent and the one that does not, and `electric-indent-mode`.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! The rules are the buffer's mode's, in `oso_core::indent`; the editor applies them at every
//! cursor as one edit (`oso_core::editor`).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

use super::edit::MAX_INSERT;

/// The indentation commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "indent-for-tab-command",
        doc: "Indent the line of every cursor as the buffer's mode says.\n\nIn Rust and TOML the line goes to its rules' column, wherever the cursor is on it: a cursor in the indentation goes to its end, one in the text keeps its place. A line already right does not change. In Markdown, Fundamental mode and internal buffers it runs `tab-to-tab-stop`.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: indent_for_tab_command,
    },
    Command {
        name: "electric-newline-and-maybe-indent",
        doc: "Insert a line break at every cursor, N times, indenting only while `electric-indent-mode` is off.\n\nSo with the mode on, as by default, it is a line break with no indentation, as Emacs's `C-j`. A negative N signals an error.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: electric_newline_and_maybe_indent,
    },
    Command {
        name: "newline-and-indent",
        doc: "Insert a line break at every cursor, N times, and indent the new line, whether `electric-indent-mode` is on or not.\n\nThe spaces and tabs around each cursor go first. A negative N signals an error.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: newline_and_indent,
    },
    Command {
        name: "electric-indent-mode",
        doc: "Toggle indenting the new line on `RET`, and the line of a typed closer.\n\nA positive prefix argument turns it on, zero or a negative one off. It is on by default; `[indent] electric` in the configuration sets it at start.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: electric_indent_mode,
    },
];

fn indent_for_tab_command(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.indent_for_tab()?;
    ctx.request_redraw();
    Ok(())
}

/// Inserts `count` line breaks at every cursor, the file's own, indenting the last new line
/// if `indent` (ADR 0115).
pub(super) fn line_breaks(
    ctx: &mut Ctx<'_>,
    count: isize,
    indent: bool,
) -> Result<(), CommandError> {
    let times = usize::try_from(count)
        .map_err(|_| CommandError::new(format!("Negative repetition argument {count}")))?;
    if times == 0 {
        return Ok(());
    }
    let ending = ctx.editor.current().1.file_info().line_ending.text();
    let cursors = ctx.editor.window().selections.len();
    let fits = ending
        .len()
        .checked_mul(times)
        .and_then(|bytes| bytes.checked_mul(cursors))
        .is_some_and(|bytes| bytes <= MAX_INSERT);
    if !fits || !ctx.editor.newline(ending, times, indent, MAX_INSERT)? {
        return Err(CommandError::new("Too much text to insert"));
    }
    ctx.request_redraw();
    Ok(())
}

fn electric_newline_and_maybe_indent(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let indent = !ctx.editor.electric_indent();
    line_breaks(ctx, args.count(), indent)
}

fn newline_and_indent(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    line_breaks(ctx, args.count(), true)
}

/// Turns `RET`'s indentation on or off as Emacs's minor modes read their argument: none
/// toggles, a positive one turns it on and zero or a negative one off. Says what it is now.
fn electric_indent_mode(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let now = match args.raw() {
        None => !ctx.editor.electric_indent(),
        Some(_) => args.count() > 0,
    };
    ctx.editor.set_electric_indent(now);
    let state = if now { "enabled" } else { "disabled" };
    ctx.editor.message(format!("Electric-Indent mode {state}"));
    Ok(())
}
