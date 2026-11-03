//! The editor's commands (ADR 0034). Each is a plain function over the
//! host-owned state in [`Ctx`], registered in [`COMMANDS`] with a docstring
//! and an argument spec. Default key bindings are in `bindings.rs`.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Registry};

/// Every command, in the order `M-x` and the command reference list them.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "forward-char",
        doc: "Move point N characters forward (backward if N is negative).",
        args: ArgSpec::Count,
        run: forward_char,
    },
    Command {
        name: "backward-char",
        doc: "Move point N characters backward (forward if N is negative).",
        args: ArgSpec::Count,
        run: backward_char,
    },
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

/// A registry with every command in [`COMMANDS`].
///
/// # Panics
///
/// If a command is registered twice or lacks a docstring. The tests build
/// this registry, so such a mistake cannot ship.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    if let Err(e) = registry.register_all(COMMANDS) {
        panic!("{e}");
    }
    registry
}

/// Moves the cursor by `n` characters, saturating at both ends, and asks for
/// a redraw if it moved.
fn move_by(ctx: &mut Ctx<'_>, n: isize) {
    let cursor = &mut ctx.editor.cursor;
    let before = cursor.position;
    cursor.position = before.saturating_add_signed(n);
    if cursor.position != before {
        ctx.request_redraw();
    }
}

fn forward_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, args.count());
    Ok(())
}

fn backward_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, args.count().saturating_neg());
    Ok(())
}

fn keyboard_quit(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    // The echo area (M2) shows "Quit"; deactivating the mark comes with it.
    log::info!("Quit");
    Ok(())
}

fn save_buffers_kill_terminal(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.request_exit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS, registry};
    use oso_core::command::{Args, Ctx, Effects};
    use oso_core::editor::Editor;

    fn run(editor: &mut Editor, name: &str, args: Args) -> Effects {
        let registry = registry();
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::new(editor);
        registry.execute(id, &mut ctx, args).unwrap();
        ctx.effects()
    }

    #[test]
    fn every_command_registers() {
        assert_eq!(registry().iter().len(), COMMANDS.len());
    }

    #[test]
    fn char_motion_moves_and_redraws() {
        let mut editor = Editor::default();
        assert!(run(&mut editor, "forward-char", Args::NONE).redraw);
        assert_eq!(editor.cursor.position, 1);
        run(&mut editor, "forward-char", Args::numeric(4));
        assert_eq!(editor.cursor.position, 5);
        run(&mut editor, "backward-char", Args::numeric(2));
        assert_eq!(editor.cursor.position, 3);
        run(&mut editor, "backward-char", Args::numeric(-1));
        assert_eq!(editor.cursor.position, 4);
        run(&mut editor, "forward-char", Args::numeric(-10));
        assert_eq!(editor.cursor.position, 0);
    }

    #[test]
    fn motion_that_does_not_move_does_not_redraw() {
        let mut editor = Editor::default();
        assert_eq!(
            run(&mut editor, "backward-char", Args::NONE),
            Effects::default()
        );
        assert_eq!(
            run(&mut editor, "forward-char", Args::numeric(0)),
            Effects::default()
        );
        editor.cursor.position = usize::MAX;
        assert_eq!(
            run(&mut editor, "forward-char", Args::NONE),
            Effects::default()
        );
        assert_eq!(
            run(&mut editor, "backward-char", Args::numeric(isize::MIN)),
            Effects::default()
        );
    }

    #[test]
    fn keyboard_quit_changes_nothing() {
        let mut editor = Editor::default();
        assert_eq!(
            run(&mut editor, "keyboard-quit", Args::NONE),
            Effects::default()
        );
    }

    #[test]
    fn save_buffers_kill_terminal_exits() {
        let mut editor = Editor::default();
        let effects = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert!(effects.exit);
        assert!(!effects.redraw);
    }
}
