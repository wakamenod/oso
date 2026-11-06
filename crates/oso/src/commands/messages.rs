//! Commands for the messages the editor keeps (ADR 0063).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
#[cfg(test)]
use oso_core::editor::Editor;

/// The commands for messages.
pub const COMMANDS: &[Command] = &[Command {
    name: "view-echo-area-messages",
    doc: "Show the `*Messages*` buffer, with its last message in view.\n\nFrom `*Messages*`, go back to where the window was, as nothing else leaves it until `C-x b` arrives.",
    args: ArgSpec::None,
    run: view_echo_area_messages,
}];

fn view_echo_area_messages(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.toggle_messages();
    ctx.request_redraw();
    Ok(())
}

/// What `*Messages*` holds, oldest first, for tests of what commands say.
#[cfg(test)]
pub(crate) fn kept_messages(editor: &Editor) -> Vec<String> {
    let id = editor.messages_buffer();
    let text = editor.buffer(id).map(|buffer| buffer.text().to_string());
    let text = text.unwrap_or_default();
    text.lines().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{cursor, editor, run};
    use oso_core::command::Args;
    use oso_core::selection::SelectionSet;

    const COMMAND: &str = "view-echo-area-messages";

    #[test]
    fn it_shows_the_messages_and_comes_back_and_draws_each_time() {
        let mut editor = editor("abcdef");
        editor.current_mut().0.selections = SelectionSet::new(3);
        editor.message("one");
        let (result, effects) = run(&mut editor, COMMAND, Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        assert_eq!(editor.current().1.name(), "*Messages*");
        let (_, effects) = run(&mut editor, COMMAND, Args::NONE);
        assert!(effects.redraw);
        assert_eq!(editor.current().1.name(), "test");
        assert_eq!(cursor(&editor), 3);
    }
}
