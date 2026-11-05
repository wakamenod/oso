//! Commands for the session as a whole: cancelling what is in progress, and
//! exiting (ADR 0034, ADR 0059).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::editor::ExitAction;

/// The session commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "keyboard-quit",
        doc: "Cancel what is in progress.\n\nCancelling a pending prefix key is handled by the key resolver.",
        args: ArgSpec::None,
        run: keyboard_quit,
    },
    Command {
        name: "save-buffers-kill-terminal",
        doc: "Exit oso, if nothing is at risk.\n\nWith a modified buffer or a save running, this refuses once and says how many. Repeating it at once confirms, and a save still running then gets to finish. Any other key resets it. Asking to save each buffer comes with prompts.",
        args: ArgSpec::None,
        run: save_buffers_kill_terminal,
    },
];

fn keyboard_quit(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    // The echo area shows "Quit" from M2-7. Deactivating the mark comes with marks.
    log::debug!("Quit");
    Ok(())
}

fn save_buffers_kill_terminal(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor
        .confirm_exit(ExitAction::Command)
        .map_err(|refused| CommandError::new(refused.to_string()))?;
    ctx.request_exit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::buffer::Replacement;
    use oso_core::command::{Args, Effects};

    #[test]
    fn keyboard_quit_changes_nothing() {
        let mut editor = editor("");
        assert_eq!(
            run(&mut editor, "keyboard-quit", Args::NONE),
            (Ok(()), Effects::default())
        );
    }

    #[test]
    fn save_buffers_kill_terminal_exits() {
        let mut editor = editor("");
        let (_, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert!(effects.exit);
        assert!(!effects.redraw);
    }

    #[test]
    fn save_buffers_kill_terminal_refuses_once_with_a_modified_buffer() {
        let mut editor = editor("");
        let id = editor.window().buffer();
        let edit = Replacement {
            range: 0..0,
            text: "x",
        };
        editor.apply_edits(id, &[edit]).unwrap();
        let (result, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert_eq!(
            result,
            Err("1 modified buffer; repeat to exit anyway".to_owned())
        );
        assert!(!effects.exit);
        let (result, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.exit);
    }
}
