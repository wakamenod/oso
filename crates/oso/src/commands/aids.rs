//! Commands that turn the visual aids on and off for every buffer (ADR 0097).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::VisualAids;

/// The commands for the visual aids. They change only what is drawn, and nothing on disk.
/// Whitespace's marks are the person's: they show lookalike spaces, so the agent channel may
/// not turn them off before the person reads a file (ADR 0097).
pub const COMMANDS: &[Command] = &[
    Command {
        name: "global-whitespace-mode",
        doc: "Toggle marking whitespace in every buffer.\n\nTabs, full-width and other non-ASCII spaces get a mark, and trailing whitespace a fill, but on a line with a cursor. A positive prefix argument turns it on, zero or a negative one off.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: global_whitespace_mode,
        state: None,
    },
    Command {
        name: "global-hl-line-mode",
        doc: "Toggle the band on the current line in every buffer.\n\nA positive prefix argument turns it on, zero or a negative one off.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: global_hl_line_mode,
        state: None,
    },
    Command {
        name: "beacon-mode",
        doc: "Toggle the cursor flash after a large jump.\n\nThe flash fades from the cursor to the window's edge after a jump of 5 rows or more, a scroll on purpose or another buffer or window. A positive prefix argument turns it on, zero or a negative one off.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: beacon_mode,
        state: None,
    },
    Command {
        name: "global-display-line-numbers-mode",
        doc: "Toggle line numbers in every buffer.\n\nAbsolute numbers in a gutter at the window's left, the current line's brighter. A positive prefix argument turns them on, zero or a negative one off.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: global_display_line_numbers_mode,
        state: None,
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

fn beacon_mode(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    switch(ctx, args, "Beacon", |aids| &mut aids.flash);
    Ok(())
}

fn global_display_line_numbers_mode(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    switch(ctx, args, "Global Display-Line-Numbers", |aids| {
        &mut aids.line_numbers
    });
    Ok(())
}

/// Sets the aid `field` picks as Emacs's minor modes read their argument: none toggles, a
/// positive one turns it on and zero or a negative one off. Says what it is now, and draws
/// if it changed.
fn switch(ctx: &mut Ctx<'_>, args: Args, mode: &str, field: fn(&mut VisualAids) -> &mut bool) {
    let mut aids = ctx.editor.visual_aids();
    let on = field(&mut aids);
    let was = *on;
    *on = super::minor_mode_on(args, was);
    let now = *on;
    ctx.editor.set_visual_aids(aids);
    ctx.editor.message(super::minor_mode_said(mode, now));
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

    #[test]
    fn beacon_mode_turns_the_flash_off_and_on() {
        let mut editor = editor("abc");
        assert!(editor.visual_aids().flash);
        let _ = run(&mut editor, "beacon-mode", Args::NONE);
        assert!(!editor.visual_aids().flash);
        let said = kept_messages(&editor);
        assert_eq!(
            said.last().map(String::as_str),
            Some("Beacon mode disabled")
        );
        let _ = run(&mut editor, "beacon-mode", Args::numeric(1));
        assert!(editor.visual_aids().flash);
    }

    #[test]
    fn line_numbers_take_the_gutter_from_the_text_area() {
        let mut editor = editor("abc");
        let columns = editor.window().size().columns;
        assert!(!editor.visual_aids().line_numbers);
        let (_, effects) = run(&mut editor, "global-display-line-numbers-mode", Args::NONE);
        assert!(effects.redraw);
        assert_eq!(editor.tiled_regions().gutter.columns, 4);
        assert_eq!(editor.window().size().columns, columns - 4);
    }
}
