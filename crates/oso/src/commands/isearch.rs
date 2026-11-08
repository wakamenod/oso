//! isearch's commands (ADR 0082): starting a search, the keys of its keymap, and its
//! exit. Where it goes, its count and its steps back are the editor's.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! A key isearch's keymap does not bind ends the search before it runs, in the
//! dispatcher, and `C-g` is `keyboard-quit`, whose chain is the editor's. Nothing here
//! logs the string: it can say something of a buffer that may hold secrets.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};

/// The commands of isearch.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "isearch-forward",
        doc: "Search forward as you type.\n\nEach character typed adds to the search string, and the cursor goes to the end of the next match. Case does not matter unless the string has an uppercase letter. `C-s` goes to the next match, round to the start after a failure, `C-r` back, `DEL` back a step, `RET` ends the search there, and `C-g` takes a failing part away, then goes back where the search began. Any other key ends the search and runs. Only the primary cursor moves.",
        args: ArgSpec::None,
        run: isearch_forward,
    },
    Command {
        name: "isearch-backward",
        doc: "Search backward as you type.\n\nAs `isearch-forward`, but the cursor goes to the start of the match before.",
        args: ArgSpec::None,
        run: isearch_backward,
    },
    Command {
        name: "isearch-repeat-forward",
        doc: "Go to the next match of the search string.\n\nAfter a failing search, start again from the start of the buffer. With no string, search for the last one.",
        args: ArgSpec::None,
        run: isearch_repeat_forward,
    },
    Command {
        name: "isearch-repeat-backward",
        doc: "Go to the match of the search string before this one.\n\nAfter a failing search, start again from the end of the buffer. With no string, search for the last one.",
        args: ArgSpec::None,
        run: isearch_repeat_backward,
    },
    Command {
        name: "isearch-printing-char",
        doc: "Add the character typed to the search string, and search again from the match.",
        args: ArgSpec::None,
        run: isearch_printing_char,
    },
    Command {
        name: "isearch-delete-char",
        doc: "Undo the last step of the search: the string and the place go back together.",
        args: ArgSpec::None,
        run: isearch_delete_char,
    },
    Command {
        name: "isearch-exit",
        doc: "End the search at the match.\n\nThe place it began is pushed onto the mark ring, unless a region shows.",
        args: ArgSpec::None,
        run: isearch_exit,
    },
    Command {
        name: "isearch-yank-word-or-char",
        doc: "Add the word after the match to the search string, or the character there if no word starts there.",
        args: ArgSpec::None,
        run: isearch_yank_word_or_char,
    },
    Command {
        name: "isearch-yank-kill",
        doc: "Add the newest kill to the search string.\n\nIt takes what the kill ring holds and does not wait for the system clipboard.",
        args: ArgSpec::None,
        run: isearch_yank_kill,
    },
    Command {
        name: "isearch-ring-retreat",
        doc: "Put the string searched for before this one in the search, and search for it.",
        args: ArgSpec::None,
        run: isearch_ring_retreat,
    },
    Command {
        name: "isearch-ring-advance",
        doc: "Put the string searched for after this one in the search, and search for it.",
        args: ArgSpec::None,
        run: isearch_ring_advance,
    },
    Command {
        name: "isearch-toggle-case-fold",
        doc: "Turn around whether case matters, for the rest of this search.",
        args: ArgSpec::None,
        run: isearch_toggle_case_fold,
    },
];

fn isearch_forward(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    begin(ctx, false)
}

fn isearch_backward(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    begin(ctx, true)
}

/// Starts a search, or repeats it when one is under way, as from `run-command`.
fn begin(ctx: &mut Ctx<'_>, backward: bool) -> Result<(), CommandError> {
    if ctx.editor.isearch().is_some() {
        ctx.editor.isearch_repeat(backward);
    } else if !ctx.editor.isearch_begin(backward) {
        return Err(CommandError::new("isearch does not work in a prompt"));
    }
    ctx.request_redraw();
    Ok(())
}

/// Runs `work` on the search under way, and asks for a frame. An error without one.
fn searching(ctx: &mut Ctx<'_>, work: impl FnOnce(&mut Ctx<'_>)) -> Result<(), CommandError> {
    if ctx.editor.isearch().is_none() {
        return Err(CommandError::new("No search under way"));
    }
    work(ctx);
    ctx.request_redraw();
    Ok(())
}

fn isearch_repeat_forward(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    searching(ctx, |ctx| ctx.editor.isearch_repeat(false))
}

fn isearch_repeat_backward(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    searching(ctx, |ctx| ctx.editor.isearch_repeat(true))
}

fn isearch_printing_char(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let input = ctx.input();
    if input.is_empty() {
        return Err(CommandError::new("No text to search for"));
    }
    searching(ctx, |ctx| ctx.editor.isearch_add(input))
}

fn isearch_delete_char(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let mut went_back = false;
    searching(ctx, |ctx| went_back = ctx.editor.isearch_delete())?;
    if went_back {
        Ok(())
    } else {
        Err(CommandError::new("No previous step in this search"))
    }
}

fn isearch_exit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    searching(ctx, |ctx| {
        ctx.editor.isearch_exit();
    })
}

fn isearch_yank_word_or_char(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    searching(ctx, |ctx| ctx.editor.isearch_yank_word())
}

fn isearch_yank_kill(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if ctx.editor.kill_ring().is_empty() {
        return Err(CommandError::new("Kill ring is empty"));
    }
    searching(ctx, |ctx| ctx.editor.isearch_yank_kill())
}

fn isearch_ring_retreat(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    searching(ctx, |ctx| ctx.editor.isearch_history(true))
}

fn isearch_ring_advance(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    searching(ctx, |ctx| ctx.editor.isearch_history(false))
}

fn isearch_toggle_case_fold(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    searching(ctx, |ctx| ctx.editor.isearch_toggle_case())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oso_core::buffer::Buffer;
    use oso_core::command::Invocation;
    use oso_core::editor::Editor;

    fn run(editor: &mut Editor, name: &str, input: &str) -> Result<bool, String> {
        let registry = crate::commands::registry();
        let id = registry.lookup(name).unwrap();
        let invocation = Invocation {
            input,
            ..Invocation::default()
        };
        let mut ctx = Ctx::invoked(editor, invocation);
        let result = registry.execute(id, &mut ctx, Args::NONE);
        let redraw = ctx.effects().redraw;
        result.map(|()| redraw).map_err(|e| e.to_string())
    }

    #[test]
    fn a_search_starts_types_and_ends() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a", "say foo"));
        assert_eq!(run(&mut editor, "isearch-forward", ""), Ok(true));
        assert_eq!(run(&mut editor, "isearch-printing-char", "fo"), Ok(true));
        assert_eq!(editor.window().selections.primary().cursor, 6);
        assert_eq!(run(&mut editor, "isearch-exit", ""), Ok(true));
        assert!(editor.isearch().is_none());
    }

    #[test]
    fn its_keys_fail_with_no_search() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a", "x"));
        for name in [
            "isearch-repeat-forward",
            "isearch-exit",
            "isearch-delete-char",
        ] {
            assert!(
                run(&mut editor, name, "")
                    .unwrap_err()
                    .contains("No search"),
                "{name}"
            );
        }
        assert!(run(&mut editor, "isearch-yank-kill", "").is_err());
    }

    #[test]
    fn delete_with_no_step_fails() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a", "x"));
        run(&mut editor, "isearch-backward", "").unwrap();
        let error = run(&mut editor, "isearch-delete-char", "").unwrap_err();
        assert!(error.contains("No previous step"));
        assert!(editor.isearch().is_some());
    }
}
