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
        doc: "Exit oso, if nothing is at risk.\n\nWith a modified buffer, a save running, or a program running in a terminal, this asks whether to exit, and says how many. `y` exits, unless more is at risk by then, when it asks again, and a save still running then gets to finish.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: save_buffers_kill_terminal,
        state: None,
    },
];

fn keyboard_quit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    // Whatever else it does, a person's `C-g` drops a search for cursors (ADR 0091).
    ctx.editor.drop_cursor_search();
    // A review comment's draft that holds text goes to the kill ring, which is said in place
    // of "Quit" (ADR 0122).
    let draft_kept = ctx.editor.draft().is_some() && !ctx.editor.current().1.text().is_empty();
    let quit = ctx.editor.quit();
    if quit != Quit::Nothing {
        ctx.request_redraw();
    }
    if quit == Quit::Prompt && draft_kept {
        ctx.editor.echo("Draft in the kill ring: C-y");
        return Ok(());
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
    // Comments of the person's not sent yet are lost with the editor until reviews are saved
    // (ADR 0122, step 6), and programs in terminals end with it (ADR 0120): one question says
    // all of it.
    let mut lost = Vec::new();
    match super::review::unsent_everywhere(ctx.editor) {
        0 => {}
        1 => lost.push("1 comment of yours will be lost".to_owned()),
        n => lost.push(format!("{n} comments of yours will be lost")),
    }
    match ctx.editor.running_terminals().count() {
        0 => {}
        1 => lost.push("1 terminal runs a program".to_owned()),
        n => lost.push(format!("{n} terminals run programs")),
    }
    let lost = (!lost.is_empty()).then(|| lost.join(", and "));
    let question = match (ctx.editor.confirm_exit(ctx.confirmed()), lost) {
        (Ok(()), None) => {
            ctx.request_exit();
            return Ok(());
        }
        (Ok(()), Some(_)) if ctx.confirmed() => {
            ctx.request_exit();
            return Ok(());
        }
        (Ok(()), Some(lost)) => format!("{lost}; exit anyway?"),
        (Err(refused), None) => format!("{refused}; exit without saving?"),
        (Err(refused), Some(lost)) => format!("{lost}, and {refused}; exit without saving?"),
    };
    ask(
        ctx,
        question,
        "save-buffers-kill-terminal",
        ["exit", "stay"],
    )
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
    fn save_buffers_kill_terminal_asks_once_while_a_terminal_runs_a_program() {
        let mut editor = editor("");
        let terminal = editor.add_terminal_buffer("*terminal*");
        editor.set_running(terminal, true);
        let (result, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(!effects.exit);
        let prompt = editor.prompt().expect("a question");
        assert_eq!(
            prompt.label(),
            "1 terminal runs a program; exit anyway? (y or n)"
        );
        editor.quit();
        let id = editor.window().buffer();
        let edit = Replacement {
            range: 0..0,
            text: "x",
        };
        editor.apply_edits(id, &[edit]).unwrap();
        let (_, effects) = run(&mut editor, "save-buffers-kill-terminal", Args::NONE);
        assert!(!effects.exit);
        assert_eq!(
            editor.prompt().expect("a question").label(),
            "1 terminal runs a program, and 1 modified buffer; exit without saving? (y or n)"
        );
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
