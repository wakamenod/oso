//! Commands that scroll the selected window by screens or lines, moving
//! cursors that would leave it (ADR 0040).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::window;

/// The scrolling commands.
pub const COMMANDS: &[Command] = &[
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
];

/// Scrolls the selected window by `lines` (the text up, toward the end, if
/// positive), as Emacs's `scroll-up` does. Cursors that would leave the
/// window move to the start of its nearest row.
fn scroll(ctx: &mut Ctx<'_>, lines: isize) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current_mut();
    let display = buffer.display();
    let row_count = display.row_count();
    let top = if lines >= 0 {
        window::scroll_up(window.scroll().top, row_count, lines.unsigned_abs())
    } else {
        window::scroll_down(window.scroll().top, lines.unsigned_abs())
    }
    .map_err(|edge| CommandError::new(edge.message()))?;
    if window.set_top(top, &display) {
        ctx.request_redraw();
    }
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

#[cfg(test)]
mod tests {
    use crate::commands::tests::{cursor, editor, run};
    use oso_core::command::{Args, Effects};

    #[test]
    fn screens_scroll_and_move_the_cursor_into_view() {
        let text: String = (0..30).map(|i| format!("{i:02}\n")).collect();
        let mut editor = editor(&text);
        let (result, effects) = run(&mut editor, "scroll-up-command", Args::NONE);
        assert!(result.is_ok() && effects.redraw);
        assert_eq!(editor.window().scroll().top, 8);
        assert_eq!(cursor(&editor), 8 * 3, "start of the new top row");
        let _ = run(&mut editor, "scroll-up-command", Args::numeric(3));
        assert_eq!(editor.window().scroll().top, 11);
        let _ = run(&mut editor, "scroll-down-command", Args::NONE);
        assert_eq!(editor.window().scroll().top, 3);
        assert_eq!(cursor(&editor), 11 * 3, "still in view");
        let _ = run(&mut editor, "scroll-down-command", Args::NONE);
        assert_eq!(editor.window().scroll().top, 0);
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
        assert_eq!(editor.window().scroll().top, 24);
        assert_eq!(
            run(&mut editor, "scroll-up-command", Args::NONE),
            (Err("End of buffer".into()), Effects::default())
        );
    }
}
