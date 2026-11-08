//! Commands of completion: `M-x`, and moving through a prompt's candidates (ADR 0076).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! The dispatcher reads `M-x`'s command name in a prompt that completes over the registry,
//! and runs that command in its place, so `execute-extended-command` itself only names what
//! it reads. The rest move the selection, which the editor keeps with the prompt.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::minibuffer::PromptError;

/// The commands of completion.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "execute-extended-command",
        doc: "Read a command's name, with completion, and run it.\n\nThe prefix argument given before goes to that command: `C-u 3 M-x forward-char` moves three characters. The commands run last come first.",
        args: ArgSpec::Command {
            prompt: "M-x",
            history: "command",
        },
        run: execute_extended_command,
    },
    Command {
        name: "next-completion",
        doc: "Select the next candidate, or the one a prefix argument says.\n\nAt the last it stays.",
        args: ArgSpec::Count,
        run: next_completion,
    },
    Command {
        name: "previous-completion",
        doc: "Select the candidate before, or the one a prefix argument says.\n\nAt the first it stays.",
        args: ArgSpec::Count,
        run: previous_completion,
    },
    Command {
        name: "first-completion",
        doc: "Select the first candidate.",
        args: ArgSpec::None,
        run: first_completion,
    },
    Command {
        name: "last-completion",
        doc: "Select the last candidate.",
        args: ArgSpec::None,
        run: last_completion,
    },
    Command {
        name: "scroll-completions-up",
        doc: "Select the candidate a page of rows further on.",
        args: ArgSpec::Count,
        run: scroll_completions_up,
    },
    Command {
        name: "scroll-completions-down",
        doc: "Select the candidate a page of rows back.",
        args: ArgSpec::Count,
        run: scroll_completions_down,
    },
    Command {
        name: "insert-completion",
        doc: "Put the selected candidate in the prompt's answer, to edit it before giving it.",
        args: ArgSpec::None,
        run: insert_completion,
    },
];

/// Reached only through `run-command`, which refuses it: the dispatcher reads the name and
/// runs that command instead.
fn execute_extended_command(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    Err(CommandError::new("Run a command through M-x"))
}

fn next_completion(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    moved(ctx, |editor| editor.move_completion(args.count()))
}

fn previous_completion(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    moved(ctx, |editor| editor.move_completion(-args.count()))
}

fn first_completion(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    moved(ctx, |editor| editor.select_completion_end(false))
}

fn last_completion(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    moved(ctx, |editor| editor.select_completion_end(true))
}

fn scroll_completions_up(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    moved(ctx, |editor| editor.page_completion(args.count()))
}

fn scroll_completions_down(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    moved(ctx, |editor| editor.page_completion(-args.count()))
}

fn insert_completion(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    moved(ctx, oso_core::editor::Editor::insert_completion)
}

/// Runs `step` on the editor, and draws what it changed. With nothing matching, it says so,
/// as the prompt does for `RET`.
fn moved(
    ctx: &mut Ctx<'_>,
    step: impl FnOnce(&mut oso_core::editor::Editor) -> Result<(), PromptError>,
) -> Result<(), CommandError> {
    match step(ctx.editor) {
        Ok(()) => {
            ctx.request_redraw();
            Ok(())
        }
        Err(PromptError::Invalid) => Err(CommandError::new("No match")),
        Err(PromptError::NoPrompt) => Err(CommandError::new("No completion here")),
        Err(error) => Err(CommandError::new(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::commands::tests::{editor, run};
    use oso_core::buffer::Replacement;
    use oso_core::command::Args;
    use oso_core::complete::Candidate;
    use oso_core::editor::Editor;
    use oso_core::minibuffer::Outcome;

    fn open(editor: &mut Editor, texts: &[&str]) {
        let items: Arc<[Candidate]> = texts.iter().map(|t| Candidate::new(*t, "", "")).collect();
        editor.open_completion("M-x", "command", items).unwrap();
    }

    fn typed(editor: &mut Editor, text: &str) {
        let id = editor.prompt().unwrap().buffer();
        let end = editor.buffer(id).unwrap().text().len();
        editor
            .apply_edits(
                id,
                &[Replacement {
                    range: end..end,
                    text,
                }],
            )
            .unwrap();
        editor.end_command();
    }

    fn selected(editor: &Editor) -> Option<&str> {
        let candidates = editor.prompt()?.candidates()?;
        candidates.selected_candidate().map(Candidate::text)
    }

    #[test]
    fn moving_stops_at_the_ends_and_takes_a_count() {
        let mut editor = editor("text");
        open(&mut editor, &["a", "bb", "ccc", "dddd"]);
        assert_eq!(selected(&editor), Some("a"));
        run(&mut editor, "next-completion", Args::numeric(2))
            .0
            .unwrap();
        assert_eq!(selected(&editor), Some("ccc"));
        run(&mut editor, "next-completion", Args::numeric(9))
            .0
            .unwrap();
        assert_eq!(selected(&editor), Some("dddd"));
        run(&mut editor, "previous-completion", Args::NONE)
            .0
            .unwrap();
        assert_eq!(selected(&editor), Some("ccc"));
        run(&mut editor, "first-completion", Args::NONE).0.unwrap();
        assert_eq!(selected(&editor), Some("a"));
        run(&mut editor, "last-completion", Args::NONE).0.unwrap();
        assert_eq!(selected(&editor), Some("dddd"));
        run(&mut editor, "scroll-completions-down", Args::NONE)
            .0
            .unwrap();
        assert_eq!(selected(&editor), Some("a"));
    }

    #[test]
    fn typing_matches_again_and_with_nothing_matching_says_so() {
        let mut editor = editor("text");
        open(&mut editor, &["save-buffer", "undo", "undo-redo"]);
        typed(&mut editor, "undo");
        assert_eq!(editor.prompt().unwrap().candidates().unwrap().len(), 2);
        typed(&mut editor, "x");
        let (result, _) = run(&mut editor, "next-completion", Args::NONE);
        assert_eq!(result.unwrap_err(), "No match");
        run(&mut editor, "exit-minibuffer", Args::NONE).0.unwrap();
        assert_eq!(editor.prompt().unwrap().message(), Some("No match"));
        assert!(editor.take_prompt_outcome().is_none());
    }

    #[test]
    fn tab_puts_the_selected_candidate_in_the_answer_and_ret_gives_it() {
        let mut editor = editor("text");
        open(&mut editor, &["save-buffer", "save-buffers-kill-terminal"]);
        typed(&mut editor, "sa");
        run(&mut editor, "next-completion", Args::NONE).0.unwrap();
        run(&mut editor, "insert-completion", Args::NONE).0.unwrap();
        let id = editor.prompt().unwrap().buffer();
        let answer = editor.buffer(id).unwrap().text().to_string();
        assert_eq!(answer, "save-buffers-kill-terminal");
        // The exact match shows first now.
        assert_eq!(selected(&editor), Some("save-buffers-kill-terminal"));
        run(&mut editor, "exit-minibuffer", Args::NONE).0.unwrap();
        let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
            panic!("no answer");
        };
        assert_eq!(text, "save-buffers-kill-terminal");
    }

    #[test]
    fn ret_on_an_empty_answer_gives_the_first_candidate_and_records_it() {
        let mut editor = editor("text");
        open(&mut editor, &["undo", "save-buffer"]);
        run(&mut editor, "next-completion", Args::NONE).0.unwrap();
        run(&mut editor, "exit-minibuffer", Args::NONE).0.unwrap();
        editor.take_prompt_outcome();
        // What was given last comes first next time.
        open(&mut editor, &["undo", "save-buffer"]);
        assert_eq!(selected(&editor), Some("save-buffer"));
        run(&mut editor, "exit-minibuffer", Args::NONE).0.unwrap();
        let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
            panic!("no answer");
        };
        assert_eq!(text, "save-buffer");
    }

    #[test]
    fn outside_a_prompt_that_completes_they_say_so() {
        let mut editor = editor("text");
        let (result, _) = run(&mut editor, "next-completion", Args::NONE);
        assert_eq!(result.unwrap_err(), "No completion here");
    }
}
