//! The completion popup's commands (ADR 0138, Completion in the buffer, step 9), as Corfu's:
//! moving its selection, with wrap, inserting the selected item, and closing it. Its keys are
//! a layer above the buffer's while it is open (`bindings::popup`); any other command closes
//! it first, and typing goes on into the buffer, narrowing it.
//!
//! Must not depend on: winit, the GPU or the host's servers; the editor keeps the popup.
//!
//! The popup is a float for the person at the keyboard, which the agent never reads, so each
//! is `Person` (ADR 0138, Q24).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The popup's commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "completion-popup-next",
        doc: "Select the completion popup's next item, from the last to the first, as Corfu's `corfu-next` with `corfu-cycle`.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: next,
        state: None,
    },
    Command {
        name: "completion-popup-previous",
        doc: "Select the completion popup's previous item, from the first to the last, as Corfu's `corfu-previous` with `corfu-cycle`.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: previous,
        state: None,
    },
    Command {
        name: "completion-popup-insert",
        doc: "Insert the completion popup's selected item in place of the word, and close it, as Corfu's `corfu-insert`.\n\nWith several cursors, it goes in at each one whose word before it is the primary's. `RET` runs it only once the selection was moved; otherwise `RET` closes the popup and breaks the line.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: insert,
        state: None,
    },
    Command {
        name: "completion-popup-quit",
        doc: "Close the completion popup, leaving the word as typed, as Corfu's `corfu-quit`.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: quit,
        state: None,
    },
];

fn next(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.completion_popup_move(1);
    ctx.request_redraw();
    Ok(())
}

fn previous(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.completion_popup_move(-1);
    ctx.request_redraw();
    Ok(())
}

fn insert(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let inserted = ctx.editor.completion_popup_insert();
    ctx.request_redraw();
    match inserted {
        Ok(()) => Ok(()),
        Err(Some(error)) => Err(CommandError::new(error.to_string())),
        Err(None) => Err(CommandError::new("No completion is selected")),
    }
}

fn quit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.close_completion_popup();
    ctx.request_redraw();
    Ok(())
}
