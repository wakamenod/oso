//! Commands that turn the visual aids on and off for every buffer (ADR 0097).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::VisualAids;

/// The commands for the visual aids. They change only what is drawn, and nothing on disk.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "global-whitespace-mode",
        doc: "Toggle marking whitespace in every buffer.\n\nTabs, full-width and other non-ASCII spaces get a mark, and trailing whitespace a fill, but on a line with a cursor. A positive prefix argument turns it on, zero or a negative one off.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: global_whitespace_mode,
    },
    Command {
        name: "global-hl-line-mode",
        doc: "Toggle the band on the current line in every buffer.\n\nA positive prefix argument turns it on, zero or a negative one off.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: global_hl_line_mode,
    },
];

fn global_whitespace_mode(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    switch(ctx, args, "Global Whitespace", |aids| &mut aids.whitespace);
    Ok(())
}

fn global_hl_line_mode(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    switch(ctx, args, "Global Hl-Line", |aids| &mut aids.current_line);
    Ok(())
}

/// Sets the aid `field` picks as Emacs's minor modes read their argument: none toggles, a
/// positive one turns it on and zero or a negative one off. Says what it is now, and draws
/// if it changed.
fn switch(ctx: &mut Ctx<'_>, args: Args, mode: &str, field: fn(&mut VisualAids) -> &mut bool) {
    let mut aids = ctx.editor.visual_aids();
    let on = field(&mut aids);
    let was = *on;
    *on = match args.raw() {
        None => !was,
        Some(_) => args.count() > 0,
    };
    let now = *on;
    ctx.editor.set_visual_aids(aids);
    let state = if now { "enabled" } else { "disabled" };
    ctx.editor.message(format!("{mode} mode {state}"));
    if now != was {
        ctx.request_redraw();
    }
}

#[cfg(test)]
mod tests {
    use crate::commands::messages::kept_messages;
    use crate::commands::tests::{editor, run};
    use oso_core::command::Args;

    #[test]
    fn with_no_argument_it_toggles_and_says_so() {
        let mut editor = editor("abc");
        assert!(editor.visual_aids().whitespace);
        let (result, effects) = run(&mut editor, "global-whitespace-mode", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        assert!(!editor.visual_aids().whitespace);
        let said = kept_messages(&editor);
        assert_eq!(
            said.last().map(String::as_str),
            Some("Global Whitespace mode disabled")
        );
        let _ = run(&mut editor, "global-whitespace-mode", Args::NONE);
        assert!(editor.visual_aids().whitespace);
    }

    #[test]
    fn an_argument_sets_it_whatever_it_was_so_a_repeat_changes_nothing() {
        let mut editor = editor("abc");
        for _ in 0..2 {
            let _ = run(&mut editor, "global-hl-line-mode", Args::numeric(0));
            assert!(!editor.visual_aids().current_line);
        }
        let (_, effects) = run(&mut editor, "global-hl-line-mode", Args::numeric(-3));
        assert!(!effects.redraw, "already off");
        let (_, effects) = run(&mut editor, "global-hl-line-mode", Args::numeric(1));
        assert!(effects.redraw);
        assert!(editor.visual_aids().current_line);
    }
}
