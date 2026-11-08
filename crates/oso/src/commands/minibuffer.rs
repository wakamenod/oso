//! Commands of a prompt: giving the answer, walking its history, and answering a
//! question (ADR 0072).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! The dispatcher reads a command's arguments in a prompt before running it, and runs a
//! question's command once it is answered, so these only end a prompt. `C-g` is
//! `keyboard-quit`, whose chain is the editor's.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::minibuffer::PromptError;

/// The commands of prompts.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "exit-minibuffer",
        doc: "Give the answer typed in the prompt to what asked for it.\n\nA number that is not one keeps the prompt open, and it says so.",
        args: ArgSpec::None,
        run: exit_minibuffer,
    },
    Command {
        name: "previous-history-element",
        doc: "Put the answer given before the one shown in the prompt.\n\nEach prompt keeps its own history, in memory only. A prompt opened while a buffer that may hold secrets is shown keeps none, and neither does a passphrase.",
        args: ArgSpec::None,
        run: previous_history_element,
    },
    Command {
        name: "next-history-element",
        doc: "Put the answer given after the one shown in the prompt.\n\nPast the newest, what was typed comes back.",
        args: ArgSpec::None,
        run: next_history_element,
    },
    Command {
        name: "answer-yes",
        doc: "Answer yes to the question asked.\n\nWhat it asked about runs, and checks again what is at stake: if more is at risk than the question said, it asks again.",
        args: ArgSpec::None,
        run: answer_yes,
    },
    Command {
        name: "answer-no",
        doc: "Answer no to the question asked.",
        args: ArgSpec::None,
        run: answer_no,
    },
];

fn exit_minibuffer(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.request_redraw();
    match ctx.editor.submit_prompt() {
        // The prompt said why, and stays.
        Ok(()) | Err(PromptError::Invalid) => Ok(()),
        Err(error) => Err(CommandError::new(error.to_string())),
    }
}

fn previous_history_element(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    walk(ctx, true, "Beginning of history; no preceding item")
}

fn next_history_element(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    walk(ctx, false, "End of history; no default available")
}

fn walk(ctx: &mut Ctx<'_>, older: bool, at_end: &'static str) -> Result<(), CommandError> {
    match ctx.editor.walk_history(older) {
        Ok(()) => {
            ctx.request_redraw();
            Ok(())
        }
        Err(PromptError::Invalid) => Err(CommandError::new(at_end)),
        Err(_) => Err(CommandError::new("No history here")),
    }
}

fn answer_yes(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    answer(ctx, true)
}

fn answer_no(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    answer(ctx, false)
}

fn answer(ctx: &mut Ctx<'_>, yes: bool) -> Result<(), CommandError> {
    ctx.editor
        .answer_question(yes)
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::buffer::Replacement;
    use oso_core::command::Args;
    use oso_core::minibuffer::{Kind, Outcome};

    #[test]
    fn exit_minibuffer_gives_the_answer_and_closes_the_prompt() {
        let mut editor = editor("text");
        let kind = Kind::Number {
            history: Some("line"),
        };
        editor.open_prompt("Go to line: ", kind).unwrap();
        let id = editor.prompt().unwrap().buffer();
        editor
            .apply_edits(
                id,
                &[Replacement {
                    range: 0..0,
                    text: "42",
                }],
            )
            .unwrap();
        assert_eq!(run(&mut editor, "exit-minibuffer", Args::NONE).0, Ok(()));
        assert!(editor.prompt().is_none());
        let Some(Outcome::Answered { depth, text }) = editor.take_prompt_outcome() else {
            panic!("no answer");
        };
        assert_eq!((depth, text.as_str()), (1, "42"));
    }

    #[test]
    fn a_number_that_is_not_one_keeps_the_prompt_and_says_so() {
        let mut editor = editor("text");
        editor
            .open_prompt("Go to line: ", Kind::Number { history: None })
            .unwrap();
        let id = editor.prompt().unwrap().buffer();
        editor
            .apply_edits(
                id,
                &[Replacement {
                    range: 0..0,
                    text: "x",
                }],
            )
            .unwrap();
        assert_eq!(run(&mut editor, "exit-minibuffer", Args::NONE).0, Ok(()));
        assert_eq!(editor.prompt().unwrap().message(), Some("Not a number"));
        assert!(editor.take_prompt_outcome().is_none());
    }

    #[test]
    fn history_walks_back_and_returns_to_what_was_typed() {
        let mut editor = editor("text");
        let kind = Kind::Number {
            history: Some("line"),
        };
        for answer in ["1", "2"] {
            editor.open_prompt("Go to line: ", kind).unwrap();
            let id = editor.prompt().unwrap().buffer();
            editor
                .apply_edits(
                    id,
                    &[Replacement {
                        range: 0..0,
                        text: answer,
                    }],
                )
                .unwrap();
            editor.submit_prompt().unwrap();
        }
        editor.open_prompt("Go to line: ", kind).unwrap();
        let id = editor.prompt().unwrap().buffer();
        editor
            .apply_edits(
                id,
                &[Replacement {
                    range: 0..0,
                    text: "9",
                }],
            )
            .unwrap();
        let shown =
            |editor: &oso_core::editor::Editor| editor.buffer(id).unwrap().text().to_string();
        run(&mut editor, "previous-history-element", Args::NONE)
            .0
            .unwrap();
        assert_eq!(shown(&editor), "2");
        run(&mut editor, "previous-history-element", Args::NONE)
            .0
            .unwrap();
        assert_eq!(shown(&editor), "1");
        let (result, _) = run(&mut editor, "previous-history-element", Args::NONE);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Beginning of history; no preceding item"
        );
        run(&mut editor, "next-history-element", Args::NONE)
            .0
            .unwrap();
        run(&mut editor, "next-history-element", Args::NONE)
            .0
            .unwrap();
        assert_eq!(shown(&editor), "9", "what was typed comes back");
        assert!(
            run(&mut editor, "next-history-element", Args::NONE)
                .0
                .is_err()
        );
    }

    #[test]
    fn answers_close_the_question_and_only_yes_runs_its_command() {
        let mut editor = editor("text");
        editor.ask("Go on?", "keyboard-quit").unwrap();
        run(&mut editor, "answer-no", Args::NONE).0.unwrap();
        assert!(editor.prompt().is_none());
        assert!(editor.take_prompt_outcome().is_none());
        editor.ask("Go on?", "keyboard-quit").unwrap();
        run(&mut editor, "answer-yes", Args::NONE).0.unwrap();
        assert!(matches!(
            editor.take_prompt_outcome(),
            Some(Outcome::Run("keyboard-quit"))
        ));
        assert!(
            run(&mut editor, "answer-yes", Args::NONE).0.is_err(),
            "no question"
        );
    }
}
