//! Whitespace cleanup (ADR 0113): the command that cleans a buffer or its regions. Cleanup on
//! save is the save's, with `[save] trim-whitespace`.
//!
//! Must not depend on: winit, the GPU or `bindings`.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The whitespace commands.
pub const COMMANDS: &[Command] = &[Command {
    name: "delete-trailing-whitespace",
    doc: "Delete the spaces and tabs at the ends of lines, and the blank lines at the end of the buffer.\n\nWith the regions shown, only their lines, and the blank lines at the end only if a region reaches them. In a Markdown file, two spaces or more after text stay, as they end a line there. Other spaces, such as U+3000, stay. It undoes as one step. A large buffer is scanned in the background, and the text changes when the scan ends.",
    args: ArgSpec::None,
    reach: Reach::Change,
    run: delete_trailing_whitespace,
}];

fn delete_trailing_whitespace(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if ctx.editor.delete_trailing_whitespace()? {
        ctx.request_redraw();
    }
    Ok(())
}
