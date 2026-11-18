//! `consult-line` (ADR 0154): a line of the buffer the selected window shows, picked from the
//! lines that match what is typed, with the buffer around the selected one beside the list.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and does not know its keys.
//!
//! The prompt, its search and the preview are the editor's and the dispatcher's; the command
//! runs once `RET` took a row. Nothing here logs what was typed or found.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The commands of a buffer's lines.
pub const COMMANDS: &[Command] = &[Command {
    name: "consult-line",
    doc: "Go to a line of the buffer, picked from the lines that match what is typed, with the buffer around the selected one beside the list.\n\nThe list starts at the cursor's line and goes round from the end to the start, so the first row, which is selected, is the nearest match at or after the cursor. With nothing typed, it lists every line that is not empty. What is typed is matched as `project-search` matches it: split at spaces, each part in any order, `=` to keep a part as typed, `!` to leave out the lines holding it, and romaji also matching its kana. Case is ignored unless an upper-case letter is typed. At most 10000 rows are listed. `RET` leaves one cursor at the line's match, with the place before on the mark ring, and `C-g` leaves the cursor where it was. A terminal's lines are searched in copy mode.",
    args: ArgSpec::BufferLines {
        history: "consult-line",
    },
    reach: Reach::Person,
    run: consult_line,
    state: None,
}];

/// Goes to the match of the row `RET` took.
fn consult_line(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    if !ctx.editor.go_to_line_hit() {
        return Err(CommandError::new("No line was picked"));
    }
    ctx.request_redraw();
    Ok(())
}
