//! Commands that change how a buffer's text is shown, never the text: soft wrap (ADR 0100).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The commands that change how text is shown.
pub const COMMANDS: &[Command] = &[Command {
    name: "toggle-truncate-lines",
    doc: "Toggle whether long lines of the current buffer are truncated at the window's edge or wrap onto more rows.\n\nWith a positive argument, truncate them; with zero or a negative one, wrap them. Each window wraps at its own width, and motion stays on whole lines.",
    args: ArgSpec::Count,
    // It changes how text is shown, never the text, a file or the clipboard (ADR 0100).
    reach: Reach::Look,
    run: toggle_truncate_lines,
}];

/// Emacs's argument: none toggles, a positive one truncates, zero or a negative one wraps.
fn toggle_truncate_lines(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let id = ctx.editor.tiled().0.buffer();
    let wraps = ctx.editor.buffer(id).is_some_and(|buffer| buffer.wraps());
    let wrap = match args.prefix() {
        None => !wraps,
        Some(count) => count <= 0,
    };
    if ctx.editor.set_wraps(id, wrap) {
        ctx.request_redraw();
    }
    ctx.editor.message(if wrap {
        "Truncate long lines disabled"
    } else {
        "Truncate long lines enabled"
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::command::{Args, Prefix};
    use oso_core::editor::{Editor, Status};

    fn wraps(editor: &Editor) -> bool {
        editor.current().1.wraps()
    }

    fn echo(editor: &Editor) -> String {
        let mut status = Status::default();
        editor.status_into(&mut status);
        status.echo
    }

    #[test]
    fn with_no_argument_it_toggles_and_says_so() {
        let mut editor = editor(&"word ".repeat(40));
        let (result, effects) = run(&mut editor, "toggle-truncate-lines", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        assert!(wraps(&editor));
        assert!(editor.window().wrap().is_some());
        assert_eq!(echo(&editor), "Truncate long lines disabled");
        let (_, effects) = run(&mut editor, "toggle-truncate-lines", Args::NONE);
        assert!(effects.redraw);
        assert!(!wraps(&editor));
        assert!(editor.window().wrap().is_none());
        assert_eq!(echo(&editor), "Truncate long lines enabled");
    }

    #[test]
    fn a_positive_argument_truncates_and_zero_or_a_negative_one_wraps() {
        let mut editor = editor("text");
        for (args, wrapped) in [
            (Args::numeric(0), true),
            (Args::numeric(-3), true),
            (Args::with_prefix(Some(Prefix::Negative)), true),
            (Args::numeric(1), false),
            (Args::with_prefix(Some(Prefix::Universal(4))), false),
            (Args::numeric(isize::MAX), false),
            (Args::numeric(isize::MIN), true),
        ] {
            let (result, _) = run(&mut editor, "toggle-truncate-lines", args);
            assert_eq!(result, Ok(()));
            assert_eq!(wraps(&editor), wrapped, "{args:?}");
        }
    }

    #[test]
    fn asked_for_what_it_is_already_it_draws_nothing() {
        let mut editor = editor("text");
        let (result, effects) = run(&mut editor, "toggle-truncate-lines", Args::numeric(1));
        assert_eq!(result, Ok(()));
        assert!(!effects.redraw, "already truncated");
        assert_eq!(echo(&editor), "Truncate long lines enabled");
    }
}
