//! The editor's commands (ADR 0034). Each is a plain function over the
//! host-owned state in [`Ctx`], registered in [`COMMANDS`] with a docstring
//! and an argument spec. Default key bindings are in `bindings.rs`.
//!
//! Motion commands move every cursor of the selected window (ADR 0007).
//! After each command the dispatcher scrolls the window to keep its cursor
//! in view (`Editor::follow_cursor`).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Registry};
use oso_core::display_map::DisplaySnapshot;
use oso_core::motion::{self, Edge};
use oso_core::selection::Selection;
use oso_core::window;

/// Every command, in the order `M-x` and the command reference list them.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "forward-char",
        doc: "Move point N characters forward (backward if N is negative).\n\nA character is what the screen shows as one: a grapheme cluster such as `が` or an emoji sequence, a tab, a line break or an escape. At the end of the buffer, signal \"End of buffer\".",
        args: ArgSpec::Count,
        run: forward_char,
    },
    Command {
        name: "backward-char",
        doc: "Move point N characters backward (forward if N is negative).\n\nCharacters are counted as in `forward-char`. At the start of the buffer, signal \"Beginning of buffer\".",
        args: ArgSpec::Count,
        run: backward_char,
    },
    Command {
        name: "next-line",
        doc: "Move point N lines down (up if N is negative), keeping the goal column.\n\nThe goal column, in display cells, is set by the first of consecutive line motions. Started at the end of a line, the motion stays at line ends. On the last line, move to the end of the buffer and signal \"End of buffer\".",
        args: ArgSpec::Count,
        run: next_line,
    },
    Command {
        name: "previous-line",
        doc: "Move point N lines up (down if N is negative), keeping the goal column.\n\nSee `next-line`. On the first line, move to the start of the buffer and signal \"Beginning of buffer\".",
        args: ArgSpec::Count,
        run: previous_line,
    },
    Command {
        name: "move-beginning-of-line",
        doc: "Move point to the beginning of the line.\n\nWith N other than 1, move N - 1 lines down first.",
        args: ArgSpec::Count,
        run: move_beginning_of_line,
    },
    Command {
        name: "move-end-of-line",
        doc: "Move point to the end of the line, before its line break.\n\nWith N other than 1, move N - 1 lines down first.",
        args: ArgSpec::Count,
        run: move_end_of_line,
    },
    Command {
        name: "beginning-of-buffer",
        doc: "Move point to the beginning of the buffer.\n\nWith a numeric argument N, move to the line after N/10 of the way from the beginning.",
        args: ArgSpec::Count,
        run: beginning_of_buffer,
    },
    Command {
        name: "end-of-buffer",
        doc: "Move point to the end of the buffer.\n\nWith a numeric argument N, move to the line after N/10 of the way from the end.",
        args: ArgSpec::Count,
        run: end_of_buffer,
    },
    Command {
        name: "scroll-up-command",
        doc: "Scroll the text up by a screen, keeping 2 lines of context.\n\nWith a numeric argument N, scroll N lines (down if N is negative). Point moves to the top line if it would leave the window. When the end of the buffer cannot scroll further up, signal \"End of buffer\".",
        args: ArgSpec::Count,
        run: scroll_up_command,
    },
    Command {
        name: "scroll-down-command",
        doc: "Scroll the text down by a screen, keeping 2 lines of context.\n\nWith a numeric argument N, scroll N lines (up if N is negative). Point moves to the bottom line if it would leave the window. At the start of the buffer, signal \"Beginning of buffer\".",
        args: ArgSpec::Count,
        run: scroll_down_command,
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

/// Moves every cursor of the selected window with `motion`. Asks for a
/// redraw if a cursor moved, and fails with the first buffer edge a cursor
/// hit, after moving them all as far as they go.
fn move_cursors(
    ctx: &mut Ctx<'_>,
    mut motion: impl FnMut(&DisplaySnapshot, &mut Selection) -> Result<(), Edge>,
) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current_mut();
    let display = buffer.display();
    let update = window
        .selections
        .update(|selection| motion(&display, selection));
    if update.changed {
        ctx.request_redraw();
    }
    match update.error {
        Some(edge) => Err(CommandError::new(edge.message())),
        None => Ok(()),
    }
}

fn forward_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_cursors(ctx, |display, s| {
        motion::move_clusters(display, s, args.count())
    })
}

fn backward_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let n = args.count().saturating_neg();
    move_cursors(ctx, |display, s| motion::move_clusters(display, s, n))
}

fn next_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_cursors(ctx, |display, s| {
        motion::move_lines(display, s, args.count())
    })
}

fn previous_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let n = args.count().saturating_neg();
    move_cursors(ctx, |display, s| motion::move_lines(display, s, n))
}

fn move_beginning_of_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_cursors(ctx, |display, s| {
        motion::move_line_start(display, s, args.count());
        Ok(())
    })
}

fn move_end_of_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_cursors(ctx, |display, s| {
        motion::move_line_end(display, s, args.count());
        Ok(())
    })
}

/// `n` tenths, clamped to 0 to 10.
fn tenths(n: isize) -> usize {
    n.clamp(0, 10) as usize
}

fn beginning_of_buffer(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    // Pushing the mark comes with marks (M2).
    move_cursors(ctx, |display, s| {
        let len = motion::buffer_end(display);
        *s = Selection::at(match args.prefix() {
            None => 0,
            Some(n) => motion::line_after_fraction(display, len, tenths(n)),
        });
        Ok(())
    })
}

fn end_of_buffer(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_cursors(ctx, |display, s| {
        let len = motion::buffer_end(display);
        *s = Selection::at(match args.prefix() {
            None => len,
            Some(n) => motion::line_after_fraction(display, len, 10 - tenths(n)),
        });
        Ok(())
    })
}

/// Scrolls the selected window by `lines` (the text up, toward the end, if
/// positive), as Emacs's `scroll-up` does. Cursors that would leave the
/// window move to the start of its nearest row.
fn scroll(ctx: &mut Ctx<'_>, lines: isize) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current_mut();
    let display = buffer.display();
    let row_count = display.row_count();
    let top = if lines >= 0 {
        window::scroll_up(window.scroll.top, row_count, lines.unsigned_abs())
    } else {
        window::scroll_down(window.scroll.top, lines.unsigned_abs())
    }
    .map_err(|edge| CommandError::new(edge.message()))?;
    window.scroll.top = top;
    let size = window.size();
    window.selections.update(|selection| {
        let row = display.offset_to_display_point(selection.cursor).row;
        if let Some(row) = window::row_in_view(row, top, size, row_count) {
            *selection = Selection::at(motion::line_start(&display, row));
        }
        Ok::<(), ()>(())
    });
    ctx.request_redraw();
    Ok(())
}

/// The lines `C-v` and `M-v` scroll: the numeric argument, or a screen
/// less the context lines.
fn screen_or_count(ctx: &Ctx<'_>, args: Args) -> isize {
    args.prefix().unwrap_or_else(|| {
        let lines = window::screen_lines(ctx.editor.window().size());
        isize::try_from(lines).unwrap_or(isize::MAX)
    })
}

fn scroll_up_command(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let lines = screen_or_count(ctx, args);
    scroll(ctx, lines)
}

fn scroll_down_command(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let lines = screen_or_count(ctx, args);
    scroll(ctx, lines.saturating_neg())
}

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
    use super::{COMMANDS, registry};
    use oso_core::buffer::Buffer;
    use oso_core::command::{Args, Ctx, Effects, ExecuteError};
    use oso_core::editor::Editor;
    use oso_core::window::ViewSize;

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("test", text));
        editor.set_view_size(ViewSize::new(10, 40));
        editor
    }

    fn run(editor: &mut Editor, name: &str, args: Args) -> (Result<(), String>, Effects) {
        let registry = registry();
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::new(editor);
        let result = registry.execute(id, &mut ctx, args).map_err(|e| match e {
            ExecuteError::Failed { error, .. } => error.message().to_owned(),
            other => panic!("{other}"),
        });
        (result, ctx.effects())
    }

    fn cursor(editor: &Editor) -> usize {
        editor.window().selections.primary().cursor
    }

    #[test]
    fn every_command_registers() {
        assert_eq!(registry().iter().len(), COMMANDS.len());
    }

    #[test]
    fn char_motion_moves_by_cluster_and_redraws() {
        let mut editor = editor("aか\u{3099}🇯🇵b");
        assert!(run(&mut editor, "forward-char", Args::NONE).1.redraw);
        assert_eq!(cursor(&editor), 1);
        let _ = run(&mut editor, "forward-char", Args::numeric(2));
        assert_eq!(cursor(&editor), 1 + 6 + 8);
        let _ = run(&mut editor, "backward-char", Args::numeric(-1));
        assert_eq!(cursor(&editor), 16);
        let _ = run(&mut editor, "backward-char", Args::numeric(3));
        assert_eq!(cursor(&editor), 1);
    }

    #[test]
    fn char_motion_at_the_ends_signals_and_does_not_redraw() {
        let mut editor = editor("ab");
        assert_eq!(
            run(&mut editor, "backward-char", Args::NONE),
            (Err("Beginning of buffer".into()), Effects::default())
        );
        assert_eq!(
            run(&mut editor, "forward-char", Args::numeric(0)),
            (Ok(()), Effects::default())
        );
        let (result, effects) = run(&mut editor, "forward-char", Args::numeric(isize::MAX));
        assert_eq!(result, Err("End of buffer".into()));
        assert!(effects.redraw, "it moved as far as it could");
        assert_eq!(cursor(&editor), 2);
        assert_eq!(
            run(&mut editor, "backward-char", Args::numeric(isize::MIN)),
            (Err("End of buffer".into()), Effects::default())
        );
    }

    #[test]
    fn line_motion_keeps_the_goal_and_signals_at_the_ends() {
        let mut editor = editor("abcd\nx\nabcd");
        let _ = run(&mut editor, "forward-char", Args::numeric(3));
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 6, "end of the short line");
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 10, "back at column 3");
        let (result, _) = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(result, Err("End of buffer".into()));
        assert_eq!(cursor(&editor), 11);
        let _ = run(&mut editor, "previous-line", Args::numeric(2));
        assert_eq!(cursor(&editor), 3, "the goal column is kept");
        let (result, _) = run(&mut editor, "previous-line", Args::NONE);
        assert_eq!(result, Err("Beginning of buffer".into()));
        assert_eq!(cursor(&editor), 0);
    }

    #[test]
    fn line_and_buffer_ends() {
        let mut editor = editor("ab\r\ncd\nef\ngh\nij\nkl\nmn\nop\nqr\nst\n");
        let _ = run(&mut editor, "move-end-of-line", Args::NONE);
        assert_eq!(cursor(&editor), 2);
        let _ = run(&mut editor, "move-beginning-of-line", Args::numeric(2));
        assert_eq!(cursor(&editor), 4);
        let _ = run(&mut editor, "end-of-buffer", Args::NONE);
        assert_eq!(cursor(&editor), 31);
        let _ = run(&mut editor, "beginning-of-buffer", Args::NONE);
        assert_eq!(cursor(&editor), 0);
        let _ = run(&mut editor, "beginning-of-buffer", Args::numeric(5));
        assert_eq!(cursor(&editor), 16, "the line after the middle");
        let _ = run(&mut editor, "end-of-buffer", Args::numeric(10));
        assert_eq!(cursor(&editor), 4, "the line after the start");
    }

    #[test]
    fn screens_scroll_and_move_the_cursor_into_view() {
        let text: String = (0..30).map(|i| format!("{i:02}\n")).collect();
        let mut editor = editor(&text);
        let (result, effects) = run(&mut editor, "scroll-up-command", Args::NONE);
        assert!(result.is_ok() && effects.redraw);
        assert_eq!(editor.window().scroll.top, 8);
        assert_eq!(cursor(&editor), 8 * 3, "start of the new top row");
        let _ = run(&mut editor, "scroll-up-command", Args::numeric(3));
        assert_eq!(editor.window().scroll.top, 11);
        let _ = run(&mut editor, "scroll-down-command", Args::NONE);
        assert_eq!(editor.window().scroll.top, 3);
        assert_eq!(cursor(&editor), 11 * 3, "still in view");
        let _ = run(&mut editor, "scroll-down-command", Args::NONE);
        assert_eq!(editor.window().scroll.top, 0);
        assert_eq!(cursor(&editor), 9 * 3, "start of the new bottom row");
        assert_eq!(
            run(&mut editor, "scroll-down-command", Args::NONE),
            (Err("Beginning of buffer".into()), Effects::default())
        );
        assert_eq!(
            run(&mut editor, "scroll-up-command", Args::numeric(-1)).0,
            Err("Beginning of buffer".into()),
            "a negative count scrolls the other way"
        );
        for _ in 0..3 {
            let _ = run(&mut editor, "scroll-up-command", Args::NONE);
        }
        assert_eq!(editor.window().scroll.top, 24);
        assert_eq!(
            run(&mut editor, "scroll-up-command", Args::NONE),
            (Err("End of buffer".into()), Effects::default())
        );
    }

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
