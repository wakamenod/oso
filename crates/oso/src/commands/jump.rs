//! Jumps by label, as avy's (ADR 0116): the commands that start one, and the keys of its
//! keymap. What a jump finds, its labels and where it goes are the editor's.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned
//! state and does not know its keys.
//!
//! The time a string is read for is the app's, which shows the labels once it passes. A key
//! the jump's keymap does not bind ends it in the dispatcher and does not run. Nothing here
//! logs the string: it can say something of a buffer that may hold secrets.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Prefix, Reach};
use oso_core::editor::Chose;
use oso_core::jump::{Kind, Stage};

use super::not_in_prompt;

/// The commands of jumps.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "avy-goto-char-timer",
        doc: "Jump to a match of characters typed, by its label.\n\nCharacters are read until a moment passes with no key, or `RET`, and their matches in view show as they are typed. Romaji also match their hiragana and katakana (migemo) unless `[search] migemo = false`. Then each match in every window shows a label, the nearest the cursor the shortest: typing a label goes there. A key that is no label ends the jump with nothing moved, as `C-g` does. Only the primary cursor moves, and the place it left goes on the mark ring. At the labels, an action's key acts on the label typed next instead: `SPC` sets the region from the cursor to it, `W` copies its whole line and `K` kills it, `Y` copies it and `T` kills it and each yanks it at every cursor, as `C-u C-y`, and `c` adds a cursor there. The cursor stays for `W`, `K`, `Y` and `T`. With `C-u`, only the selected window is labelled.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: avy_goto_char_timer,
        state: None,
    },
    Command {
        name: "avy-goto-line",
        doc: "Jump to the start of a line in view, by its label.\n\nEach line in every window shows a label, and an action's key acts on the label typed next, as in `avy-goto-char-timer`. A digit that is no label ends the jump and reads a line number for `goto-line`, starting with it. With `C-u`, only the selected window is labelled.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: avy_goto_line,
        state: None,
    },
    Command {
        name: "avy-goto-end-of-line",
        doc: "Jump to the end of a line in view, by its label.\n\nAn action's key acts on the label typed next, as in `avy-goto-char-timer`. With `C-u`, only the selected window is labelled.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: avy_goto_end_of_line,
        state: None,
    },
    Command {
        name: "avy-isearch",
        doc: "In isearch, jump to a match in view by its label, and end the search there.\n\nThe search ends as `RET` ends it: its string goes in the history, and the place it began on the mark ring. An action's key acts on the label typed next, as in `avy-goto-char-timer`, after the search ended so.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: avy_isearch,
        state: None,
    },
    Command {
        name: "avy-printing-char",
        doc: "In a jump, add the character typed to the string it reads, or type a label's key, or an action's: `SPC`, `W`, `K`, `Y`, `T` or `c`.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: avy_printing_char,
        state: None,
    },
    Command {
        name: "avy-delete-char",
        doc: "In a jump, take the last character off the string it reads.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: avy_delete_char,
        state: None,
    },
    Command {
        name: "avy-show-labels",
        doc: "In a jump, show the labels now, without waiting for the moment to pass.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: avy_show_labels,
        state: None,
    },
];

fn avy_goto_char_timer(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    begin(ctx, Kind::Char, args)
}

fn avy_goto_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    begin(ctx, Kind::LineStart, args)
}

fn avy_goto_end_of_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    begin(ctx, Kind::LineEnd, args)
}

fn avy_isearch(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if ctx.editor.isearch().is_none() {
        return Err(CommandError::new("No search under way"));
    }
    begin(ctx, Kind::Isearch, Args::NONE)
}

/// Starts a jump of `kind`, in the selected window only with `C-u`.
fn begin(ctx: &mut Ctx<'_>, kind: Kind, args: Args) -> Result<(), CommandError> {
    not_in_prompt(ctx)?;
    if ctx.editor.jump().is_some() {
        return Err(CommandError::new("A jump is under way"));
    }
    let selected_only = matches!(args.raw(), Some(Prefix::Universal(_)));
    if !ctx.editor.jump_begin(kind, selected_only) {
        return Err(CommandError::new("Not in the minibuffer"));
    }
    ctx.request_redraw();
    Ok(())
}

/// Runs `work` on the jump under way, and asks for a frame. An error without one.
fn jumping(ctx: &mut Ctx<'_>, work: impl FnOnce(&mut Ctx<'_>)) -> Result<(), CommandError> {
    if ctx.editor.jump().is_none() {
        return Err(CommandError::new("No jump under way"));
    }
    work(ctx);
    ctx.request_redraw();
    Ok(())
}

fn avy_printing_char(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let input = ctx.input();
    let Some(key) = input.chars().next() else {
        return Err(CommandError::new("No character typed"));
    };
    let mut acted = Ok(());
    jumping(ctx, |ctx| {
        match ctx.editor.jump().map(|jump| jump.stage()) {
            Some(Stage::Reading) => ctx.editor.jump_add(input),
            Some(Stage::Labels) => {
                // What it did shows: the labels left, the cursor moved, the action's work, or
                // the message. An action that could not act changed nothing, and says why.
                acted = ctx.editor.jump_choose(key).map(|_: Chose| ());
            }
            None => {}
        }
    })?;
    acted.map_err(|error| CommandError::new(error.to_string()))
}

fn avy_delete_char(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    // At the labels it ends the jump as any other key there does, which says so itself.
    let labels = ctx.editor.jump().map(|jump| jump.stage()) == Some(Stage::Labels);
    let mut went_back = false;
    jumping(ctx, |ctx| went_back = ctx.editor.jump_delete())?;
    if went_back || labels {
        Ok(())
    } else {
        Err(CommandError::new("Nothing to delete"))
    }
}

fn avy_show_labels(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    jumping(ctx, |ctx| ctx.editor.jump_show_labels())
}

#[cfg(test)]
mod tests {
    use crate::commands::registry;
    use oso_core::buffer::Buffer;
    use oso_core::command::{Args, Ctx, Prefix};
    use oso_core::editor::Editor;
    use oso_core::frame::FrameSize;
    use oso_core::jump::{Kind, Stage};

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
        editor.set_frame_size(FrameSize {
            rows: 12,
            columns: 40,
        });
        editor
    }

    fn run(editor: &mut Editor, name: &str, args: Args) -> Result<(), String> {
        let registry = registry();
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::new(editor);
        registry
            .execute(id, &mut ctx, args)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn each_jump_starts_and_c_u_keeps_to_the_selected_window() {
        let mut editor = editor("a\nb\n");
        run(&mut editor, "avy-goto-line", Args::NONE).unwrap();
        let jump = editor.jump().unwrap();
        assert_eq!(
            (jump.kind(), jump.stage()),
            (Kind::LineStart, Stage::Labels)
        );
        assert_eq!(
            run(&mut editor, "avy-goto-line", Args::NONE),
            Err("avy-goto-line: A jump is under way".into())
        );
        editor.jump_end();
        run(&mut editor, "avy-goto-end-of-line", Args::NONE).unwrap();
        assert_eq!(editor.jump().unwrap().kind(), Kind::LineEnd);
        editor.jump_end();
        let universal = Args::with_prefix(Some(Prefix::universal(None)));
        run(&mut editor, "avy-goto-char-timer", universal).unwrap();
        assert_eq!(editor.jump().unwrap().stage(), Stage::Reading);
    }

    #[test]
    fn avy_isearch_needs_a_search_and_the_keys_need_a_jump() {
        let mut editor = editor("a\n");
        assert_eq!(
            run(&mut editor, "avy-isearch", Args::NONE),
            Err("avy-isearch: No search under way".into())
        );
        for name in ["avy-delete-char", "avy-show-labels"] {
            let error = run(&mut editor, name, Args::NONE).unwrap_err();
            assert!(error.ends_with(": No jump under way"), "{error}");
        }
    }
}
