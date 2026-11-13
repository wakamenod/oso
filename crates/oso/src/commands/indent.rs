//! The indentation commands of ADR 0115: `TAB` reindenting by the buffer's mode, the line
//! breaks, `RET`'s `newline` among them, that indent and the one that does not,
//! `electric-indent-mode`, the region's `indent-region` and `indent-rigidly` with the commands
//! of its layer, `back-to-indentation`, `delete-indentation`, Markdown's new list item and
//! `describe-indentation` (`describe`).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! The rules are the buffer's mode's, in `oso_core::indent`; the editor applies them at every
//! cursor as one edit (`oso_core::editor`).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{IndentError, Shift};

use super::MAX_INSERT;

mod describe;

/// The indentation commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "newline",
        doc: "Insert a line break at every cursor, N times, and with `electric-indent-mode` on, indent the new line.\n\nThe line break is the one the file uses, so `\\r\\n` in a CRLF file. While indenting, the spaces and tabs around each cursor go, the last new line takes the indentation its mode's rules give, and between an opener and its closer in Rust and TOML the closer goes to a line of its own. Each is an undo step of its own. A negative N signals an error.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: newline,
        state: None,
    },
    Command {
        name: "indent-for-tab-command",
        doc: "Indent the line of every cursor as the buffer's mode says.\n\nIn Rust and TOML the line goes to its rules' column, wherever the cursor is on it: a cursor in the indentation goes to its end, one in the text keeps its place. A line already right does not change. With a region shown, it runs `indent-region`. In Markdown the line cycles through the columns of the list items it could belong to, innermost first, then the margin; in a fenced code block, and in Fundamental mode and internal buffers, it runs `tab-to-tab-stop`.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: indent_for_tab_command,
        state: None,
    },
    Command {
        name: "electric-newline-and-maybe-indent",
        doc: "Insert a line break at every cursor, N times, indenting only while `electric-indent-mode` is off.\n\nSo with the mode on, as by default, it is a line break with no indentation, as Emacs's `C-j`. A negative N signals an error.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: electric_newline_and_maybe_indent,
        state: None,
    },
    Command {
        name: "newline-and-indent",
        doc: "Insert a line break at every cursor, N times, and indent the new line, whether `electric-indent-mode` is on or not.\n\nThe spaces and tabs around each cursor go first. A negative N signals an error.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: newline_and_indent,
        state: None,
    },
    Command {
        name: "electric-indent-mode",
        doc: "Toggle indenting the new line on `RET`, and the line of a typed closer.\n\nA positive prefix argument turns it on, zero or a negative one off. It is on by default; `[indent] electric` in the configuration sets it at start.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: electric_indent_mode,
        state: None,
    },
    Command {
        name: "indent-region",
        doc: "Reindent every line in each cursor's region, shown or not, as the buffer's mode says.\n\nWith a numeric argument N, put every line at column N instead. A line of only spaces and tabs becomes empty, unless it is inside a string. It works in Rust and TOML, and with N in any mode. A region too large to reindent at once signals and changes nothing; reindent it in parts.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: indent_region,
        state: None,
    },
    Command {
        name: "indent-rigidly",
        doc: "Shift every line in each cursor's region by N columns, to the left when N is negative.\n\nNo line goes past column 0, and a line of only spaces and tabs becomes empty. With no N, from a key or `M-x`, `<left>` and `<right>` then shift by one column and `S-<left>` and `S-<right>` to the next tab stop, until another key; those keys undo as one step. From the agent channel with no N, it shifts by the buffer's indent size.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: indent_rigidly,
        state: None,
    },
    Command {
        name: "indent-rigidly-left",
        doc: "Shift every line in each cursor's region one column to the left.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: indent_rigidly_left,
        state: None,
    },
    Command {
        name: "indent-rigidly-right",
        doc: "Shift every line in each cursor's region one column to the right.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: indent_rigidly_right,
        state: None,
    },
    Command {
        name: "indent-rigidly-left-to-tab-stop",
        doc: "Shift the lines of each cursor's region left, so that the least indented one reaches the tab stop before it.\n\nTab stops are every indent size columns.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: indent_rigidly_left_to_tab_stop,
        state: None,
    },
    Command {
        name: "indent-rigidly-right-to-tab-stop",
        doc: "Shift the lines of each cursor's region right, so that the least indented one reaches the next tab stop.\n\nTab stops are every indent size columns.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: indent_rigidly_right_to_tab_stop,
        state: None,
    },
    Command {
        name: "back-to-indentation",
        doc: "Move every cursor to the first character of its line that is not a space or a tab.\n\nA full-width space is text, so a cursor stops before it.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: back_to_indentation,
        state: None,
    },
    Command {
        name: "delete-indentation",
        doc: "Join this line to the one before, leaving one space between, or none at a line's edge, after an opening bracket or before a closing one.\n\nWith a prefix argument, join the next line to this one. With a region shown and no prefix argument, join all of its lines.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: delete_indentation,
        state: None,
    },
    Command {
        name: "markdown-insert-list-item",
        doc: "Start a new list item after the current one, with the same marker and indentation.\n\nIn a numbered list its number is one higher. The line breaks at point, which goes to the new item's text. Markdown only.",
        args: ArgSpec::None,
        reach: Reach::Change,
        run: markdown_insert_list_item,
        state: None,
    },
    Command {
        name: "describe-indentation",
        doc: "Say the buffer's indentation setting, and where each part of it came from.\n\nEach of the indent size, spaces or tabs, and the tab width comes from the EditorConfig files that apply to the file, over the configuration's `[modes.<mode>]`, over the default. An EditorConfig file is named by its path in the project; one outside it is not named. A tab width read since the file was visited applies at the next visit.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: describe::describe_indentation,
        state: None,
    },
];

/// The message of an indentation command that changed nothing.
fn failed(error: IndentError) -> CommandError {
    CommandError::new(error.to_string())
}

fn indent_for_tab_command(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let set = &ctx.editor.window().selections;
    let shown = set.mark_active().is_active() && set.iter().any(|s| !s.range().is_empty());
    // A mode with no rules to reindent a region by indents as with none shown.
    match shown.then(|| ctx.editor.indent_region(None, MAX_INSERT)) {
        Some(Err(IndentError::NoRules)) | None => ctx.editor.indent_for_tab()?,
        Some(done) => done.map_err(failed)?,
    }
    ctx.request_redraw();
    Ok(())
}

fn indent_region(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let column = args.prefix().map(|n| usize::try_from(n).unwrap_or(0));
    ctx.editor
        .indent_region(column, MAX_INSERT)
        .map_err(failed)?;
    ctx.request_redraw();
    Ok(())
}

/// With no count, from a person, opens the layer whose arrows shift the region; from the agent
/// channel, shifts by the indent size.
fn indent_rigidly(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let columns = match args.prefix() {
        Some(n) => n,
        None if ctx.editor.runs_for_agent() => {
            let size = ctx.editor.current().1.indent_settings().indent_size();
            isize::try_from(size).unwrap_or(0)
        }
        None => {
            if ctx
                .editor
                .window()
                .selections
                .iter()
                .any(|s| s.mark.is_none())
            {
                return Err(failed(IndentError::NoMark));
            }
            ctx.editor.set_indent_rigidly_layer(true);
            ctx.editor
                .echo("Indent region with <left>, <right>, S-<left>, or S-<right>.");
            return Ok(());
        }
    };
    shift_rigidly(ctx, columns)
}

fn shift_rigidly(ctx: &mut Ctx<'_>, columns: isize) -> Result<(), CommandError> {
    ctx.editor
        .indent_rigidly(columns, MAX_INSERT)
        .map_err(failed)?;
    ctx.request_redraw();
    Ok(())
}

fn indent_rigidly_left(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    shift_rigidly(ctx, -1)
}

fn indent_rigidly_right(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    shift_rigidly(ctx, 1)
}

fn to_tab_stop(ctx: &mut Ctx<'_>, shift: Shift) -> Result<(), CommandError> {
    ctx.editor
        .indent_rigidly_to_tab_stop(shift, MAX_INSERT)
        .map_err(failed)?;
    ctx.request_redraw();
    Ok(())
}

fn indent_rigidly_left_to_tab_stop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    to_tab_stop(ctx, Shift::Left)
}

fn indent_rigidly_right_to_tab_stop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    to_tab_stop(ctx, Shift::Right)
}

fn back_to_indentation(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::mark::shift_select(ctx);
    if ctx.editor.back_to_indentation() {
        ctx.request_redraw();
    }
    Ok(())
}

fn delete_indentation(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    ctx.editor
        .delete_indentation(args.prefix().is_some())
        .map_err(failed)?;
    ctx.request_redraw();
    Ok(())
}

fn markdown_insert_list_item(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let ending = ctx.editor.current().1.file_info().line_ending.text();
    ctx.editor
        .markdown_insert_list_item(ending, MAX_INSERT)
        .map_err(failed)?;
    ctx.request_redraw();
    Ok(())
}

/// Inserts `count` line breaks at every cursor, the file's own, indenting the last new line
/// if `indent` (ADR 0115).
fn line_breaks(ctx: &mut Ctx<'_>, count: isize, indent: bool) -> Result<(), CommandError> {
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

fn newline(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let indent = ctx.editor.electric_indent();
    line_breaks(ctx, args.count(), indent)
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
    let now = super::minor_mode_on(args, ctx.editor.electric_indent());
    ctx.editor.set_electric_indent(now);
    ctx.editor
        .message(super::minor_mode_said("Electric-Indent", now));
    Ok(())
}
