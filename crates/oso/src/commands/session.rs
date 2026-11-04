//! Commands for the session as a whole: cancelling what is in progress, and
//! exiting (ADR 0034).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};

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
        doc: "Exit oso.\n\nNothing can be unsaved yet; asking to save buffers comes with editing.",
        args: ArgSpec::None,
        run: save_buffers_kill_terminal,
    },
];

fn keyboard_quit(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    // The echo area (M2) shows "Quit"; deactivating the mark comes with it.
    log::debug!("Quit");
    Ok(())
}

fn save_buffers_kill_terminal(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.request_exit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
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
}
