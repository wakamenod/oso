//! isearch's commands (ADR 0082, ADR 0116): starting a search, literal or a regexp's, the
//! keys of its keymap, migemo's among them, and its exit. Where it goes, its count and its
//! steps back are the editor's.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! A key isearch's keymap does not bind ends the search before it runs, in the
//! dispatcher, and `C-g` is `keyboard-quit`, whose chain is the editor's. Nothing here
//! logs the string: it can say something of a buffer that may hold secrets.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::Toward;

/// The commands of isearch.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "isearch-forward",
        doc: "Search forward as you type.\n\nEach character typed adds to the search string, and the cursor goes to the end of the next match. Case does not matter unless the string has an uppercase letter. A string of romaji also matches its hiragana and katakana (migemo) unless `[search] migemo = false`, and `M-m` turns that for this search. `C-s` goes to the next match, round to the start after a failure, `C-r` back, `DEL` back a step, `RET` ends the search there, and `C-g` takes a failing part away, then goes back where the search began. Any other key ends the search and runs. Only the primary cursor moves.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_forward,
        state: None,
    },
    Command {
        name: "isearch-backward",
        doc: "Search backward as you type.\n\nAs `isearch-forward`, but the cursor goes to the start of the match before.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_backward,
        state: None,
    },
    Command {
        name: "isearch-forward-regexp",
        doc: "Search forward for a regular expression as you type.\n\nAs `isearch-forward`, but the string is a regexp in the syntax of Rust's `regex` crate, matched within one line. Case does not matter unless the regexp has an uppercase letter outside an escape, and `(?i)` or `(?-i)` in it wins. While it is incomplete, the line says so and the cursor stays.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_forward_regexp,
        state: None,
    },
    Command {
        name: "isearch-backward-regexp",
        doc: "Search backward for a regular expression as you type.\n\nAs `isearch-forward-regexp`, but the cursor goes to the start of the match that starts last before it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_backward_regexp,
        state: None,
    },
    Command {
        name: "isearch-repeat-forward",
        doc: "Go to the next match of the search string.\n\nAfter a failing search, start again from the start of the buffer. With no string, search for the last one.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_repeat_forward,
        state: None,
    },
    Command {
        name: "isearch-repeat-backward",
        doc: "Go to the match of the search string before this one.\n\nAfter a failing search, start again from the end of the buffer. With no string, search for the last one.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_repeat_backward,
        state: None,
    },
    Command {
        name: "isearch-printing-char",
        doc: "Add the character typed to the search string, and search again from the match.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_printing_char,
        state: None,
    },
    Command {
        name: "isearch-newline",
        doc: "Add a line break to the search string and search again, as `C-j` does in an Emacs search.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_newline,
        state: None,
    },
    Command {
        name: "isearch-delete-char",
        doc: "Undo the last step of the search: the string and the place go back together.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_delete_char,
        state: None,
    },
    Command {
        name: "isearch-exit",
        doc: "End the search at the match.\n\nThe place it began is pushed onto the mark ring, unless a region shows.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_exit,
        state: None,
    },
    Command {
        name: "isearch-yank-word-or-char",
        doc: "Add the word after the match to the search string, or the character there if no word starts there.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_yank_word_or_char,
        state: None,
    },
    Command {
        name: "isearch-yank-kill",
        doc: "Add the newest kill to the search string.\n\nIt takes what the kill ring holds and does not wait for the system clipboard.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_yank_kill,
        state: None,
    },
    Command {
        name: "isearch-yank-pop",
        doc: "Right after `C-y` or this, put the next older kill in place of what it added.\n\nKills from a buffer that may hold secrets are passed over.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_yank_pop,
        state: None,
    },
    Command {
        name: "isearch-ring-retreat",
        doc: "Put the string searched for before this one in the search, and search for it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_ring_retreat,
        state: None,
    },
    Command {
        name: "isearch-ring-advance",
        doc: "Put the string searched for after this one in the search, and search for it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_ring_advance,
        state: None,
    },
    Command {
        name: "isearch-toggle-case-fold",
        doc: "Turn around whether case matters, for the rest of this search.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_toggle_case_fold,
        state: None,
    },
    Command {
        name: "isearch-toggle-regexp",
        doc: "Turn the search string from literal to a regular expression or back, for the rest of this search.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_toggle_regexp,
        state: None,
    },
    Command {
        name: "isearch-toggle-migemo",
        doc: "Turn migemo off or on for the rest of this search.\n\nWith migemo, a string of lowercase letters, `'` and `-` also finds the kana it spells, in hiragana, katakana and half-width katakana, and its full-width letters: `ka` finds `か` and `カ`. A regexp's search never uses it. `[search] migemo` in the configuration file says how each search starts.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_toggle_migemo,
        state: None,
    },
    Command {
        name: "isearch-mark-next",
        doc: "End the search with the current match as the region, and add a cursor at the next match.\n\nAs `mark-next-like-this` goes on after it, with the search's string and case.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_mark_next,
        state: None,
    },
    Command {
        name: "isearch-mark-previous",
        doc: "End the search with the current match as the region, and add a cursor at the previous match.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_mark_previous,
        state: None,
    },
    Command {
        name: "isearch-mark-all",
        doc: "End the search, and put a cursor on every match in the buffer.\n\nThe primary stays on the current match. With more than 1000 counted, the search goes on and none is added.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: isearch_mark_all,
        state: None,
    },
];

fn isearch_forward(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    begin(ctx, false, false)
}

fn isearch_backward(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    begin(ctx, true, false)
}

fn isearch_forward_regexp(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    begin(ctx, false, true)
}

fn isearch_backward_regexp(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    begin(ctx, true, true)
}

/// Starts a search, of a `regexp` or a literal string, or repeats it when one is under way,
/// as from `run-command`.
fn begin(ctx: &mut Ctx<'_>, backward: bool, regexp: bool) -> Result<(), CommandError> {
    if ctx.editor.isearch().is_some() {
        ctx.editor.isearch_repeat(backward);
    } else if !ctx.editor.isearch_begin(backward, regexp) {
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

fn isearch_newline(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    searching(ctx, |ctx| ctx.editor.isearch_add("\n"))
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

fn isearch_yank_pop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let mut done = Ok(());
    searching(ctx, |ctx| done = ctx.editor.isearch_yank_pop())?;
    done.map_err(|error| CommandError::new(error.to_string()))
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

fn isearch_toggle_regexp(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    searching(ctx, |ctx| ctx.editor.isearch_toggle_regexp())
}

fn isearch_toggle_migemo(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    searching(ctx, |ctx| ctx.editor.isearch_toggle_migemo())
}

fn isearch_mark_next(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    mark(ctx, Some(Toward::Next))
}

fn isearch_mark_previous(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    mark(ctx, Some(Toward::Previous))
}

fn isearch_mark_all(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    mark(ctx, None)
}

/// Adds cursors from the search's current match (ADR 0091).
fn mark(ctx: &mut Ctx<'_>, toward: Option<Toward>) -> Result<(), CommandError> {
    let done = ctx.editor.isearch_mark(toward);
    ctx.request_redraw();
    done.map_err(|error| CommandError::new(error.to_string()))
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
    fn a_regexp_search_starts_and_its_toggle_turns_it_literal() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a", "a.c abc"));
        assert_eq!(run(&mut editor, "isearch-forward-regexp", ""), Ok(true));
        assert!(editor.isearch().is_some_and(|search| search.regexp()));
        assert_eq!(run(&mut editor, "isearch-printing-char", "b."), Ok(true));
        assert_eq!(editor.window().selections.primary().cursor, 7);
        assert_eq!(run(&mut editor, "isearch-toggle-regexp", ""), Ok(true));
        assert!(editor.isearch().is_some_and(|search| !search.regexp()));
        // `b.` literally is nowhere.
        assert!(editor.isearch().is_some_and(|search| search.failing()));
        // `C-M-s` in a search repeats it.
        assert_eq!(run(&mut editor, "isearch-backward-regexp", ""), Ok(true));
        assert!(editor.isearch().is_some_and(|search| search.backward()));
    }

    #[test]
    fn its_keys_fail_with_no_search() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a", "x"));
        for name in [
            "isearch-repeat-forward",
            "isearch-exit",
            "isearch-delete-char",
            "isearch-toggle-regexp",
            "isearch-toggle-migemo",
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
