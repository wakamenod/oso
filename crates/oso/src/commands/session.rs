//! Commands for the session as a whole: cancelling what is in progress, and
//! exiting (ADR 0034, ADR 0059, ADR 0071).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::Quit;

/// The session commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "keyboard-quit",
        doc: "Cancel what is in progress, one thing at a time.\n\nA pending prefix key or prefix argument goes first, in the key resolver. Then the region is hidden, then the innermost prompt or question closes, and runs nothing, and then isearch takes away a failing part, or else ends where it began.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: keyboard_quit,
        state: None,
    },
    Command {
        name: "save-buffers-kill-terminal",
        doc: "Exit oso, if nothing is at risk.\n\nWith a modified buffer or a save running, this asks whether to exit without saving, and says how many. `y` exits, unless more is at risk by then, when it asks again, and a save still running then gets to finish.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: save_buffers_kill_terminal,
        state: None,
    },
];

fn keyboard_quit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    // Whatever else it does, a person's `C-g` drops a search for cursors (ADR 0091).
    ctx.editor.drop_cursor_search();
    let quit = ctx.editor.quit();
    if quit != Quit::Nothing {
        ctx.request_redraw();
    }
    // The search goes on, and its line stays (ADR 0082).
    if quit == Quit::SearchStep {
        return Ok(());
    }
    // With nothing else to do, it drops a keyboard macro being recorded (ADR 0113).
    if quit == Quit::Nothing && ctx.editor.macro_recording() {
        ctx.editor
            .macro_cancel("Keyboard macro recording cancelled");
        ctx.request_redraw();
        return Ok(());
    }
    log::debug!("Quit");
    ctx.editor.echo("Quit");
    Ok(())
}

fn save_buffers_kill_terminal(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    match ctx.editor.confirm_exit(ctx.confirmed()) {
        Ok(()) => ctx.request_exit(),
        Err(refused) => {
            ask(
                ctx,
                format!("{refused}; exit without saving?"),
                "save-buffers-kill-terminal",
                ["exit", "stay"],
            )?;
        }
    }
    Ok(())
}

/// Asks `question`, whose `y` runs `command`, and shows it, with `verbs` saying what `y`
/// and `n` do (ADR 0075).
pub(super) fn ask(
    ctx: &mut Ctx<'_>,
    question: String,
    command: &'static str,
    verbs: [&'static str; 2],
) -> Result<(), CommandError> {
    ctx.editor
        .ask_with(question, command, verbs)
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::buffer::Replacement;
    use oso_core::command::{Args, Effects};
    use oso_core::minibuffer::Kind;

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
    fn save_buffers_kill_terminal_asks_with_a_modified_buffer() {
        let mut editor = editor("");
        let id = editor.window().buffer();
        let edit = Replacement {
            range: 0..0,
            text: "x",
        };
        editor.apply_edits(id, &[edit]).unwrap();
        let (result, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(!effects.exit);
        let prompt = editor.prompt().expect("a question");
        assert_eq!(
            prompt.label(),
            "1 modified buffer; exit without saving? (y or n)"
        );
        assert_eq!(
            prompt.kind(),
            Kind::Question {
                yes: "save-buffers-kill-terminal",
                verbs: ["exit", "stay"],
            }
        );
    }

    #[test]
    fn keyboard_quit_closes_the_innermost_prompt_and_runs_nothing() {
        let mut editor = editor("");
        editor.ask("First?", "keyboard-quit").unwrap();
        editor.ask("Second?", "keyboard-quit").unwrap();
        assert_eq!(editor.prompt().unwrap().label(), "[2] Second? (y or n)");
        let (_, effects) = run(&mut editor, "keyboard-quit", Args::NONE);
        assert!(effects.redraw);
        assert_eq!(editor.prompt().unwrap().label(), "First? (y or n)");
        assert!(editor.take_prompt_outcome().is_none());
        run(&mut editor, "keyboard-quit", Args::NONE).0.unwrap();
        assert!(editor.prompt().is_none());
    }
}
