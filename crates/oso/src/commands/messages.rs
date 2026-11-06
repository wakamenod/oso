//! Commands for the messages the editor keeps (ADR 0063).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};

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

#[cfg(test)]
mod tests {
    use crate::commands::tests::{cursor, editor, run};
    use oso_core::command::Args;
    use oso_core::editor::MESSAGES;
    use oso_core::selection::SelectionSet;

    const COMMAND: &str = "view-echo-area-messages";

    #[test]
    fn it_shows_the_messages_and_comes_back_to_the_same_place() {
        let mut editor = editor("abcdef");
        editor.current_mut().0.selections = SelectionSet::new(3);
        editor.message("one");
        let (result, effects) = run(&mut editor, COMMAND, Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        assert_eq!(editor.current().1.name(), MESSAGES);
        assert_eq!(editor.current().1.text().to_string(), "one\n");
        assert_eq!(cursor(&editor), 4, "at the end");

        let (_, effects) = run(&mut editor, COMMAND, Args::NONE);
        assert!(effects.redraw);
        assert_eq!(editor.current().1.name(), "test");
        assert_eq!(cursor(&editor), 3);
    }

    #[test]
    fn messages_added_while_it_is_shown_keep_the_cursor_at_the_end() {
        let mut editor = editor("");
        let _ = run(&mut editor, COMMAND, Args::NONE);
        editor.message("one");
        editor.message("two");
        assert_eq!(cursor(&editor), "one\ntwo\n".len());
    }

    #[test]
    fn it_comes_back_to_scratch_if_the_buffer_has_gone() {
        let mut editor = editor("abc");
        let id = editor.window().buffer();
        let _ = run(&mut editor, COMMAND, Args::NONE);
        editor.kill_buffer(id);
        let _ = run(&mut editor, COMMAND, Args::NONE);
        assert_eq!(editor.current().1.name(), "*scratch*");
    }
}
