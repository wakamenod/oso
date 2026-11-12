//! Keyboard macros (ADR 0113): starting and ending a recording, and replaying the last macro.
//! All are `Person`: a replay types keys, which reach every binding.
//!
//! Must not depend on: winit, the GPU or `bindings`. The dispatcher records and replays.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The keyboard macro commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "kmacro-start-macro",
        doc: "Start recording a keyboard macro.\n\nThe keys typed and the text an input method commits are recorded, until `C-x )` ends it. The macro commands' own keys are not, and a passphrase prompt drops the recording. `C-g` with nothing else to do drops it too, and the last macro stays.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: start,
    },
    Command {
        name: "kmacro-end-macro",
        doc: "End recording a keyboard macro.\n\nWith a count N, also run it N - 1 more times.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: end,
    },
    Command {
        name: "kmacro-end-and-call-macro",
        doc: "Run the last keyboard macro, ending a recording first.\n\nWith a count N, run it N times, and with 0, until something stops it. `e` right after runs it again. A command's error, an undefined key, a failing search, a question or a passphrase prompt stops it, and `C-g` does. What it changes in a buffer undoes as one step.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: end_and_call,
    },
];

/// The times a count asks for: `None`, as many as run until something stops it, for 0.
fn times(args: Args) -> Option<usize> {
    match args.count() {
        0 => None,
        n => Some(n.unsigned_abs()),
    }
}

fn start(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor
        .macro_start()
        .map_err(|refused| CommandError::new(refused.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

fn end(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    ctx.editor
        .macro_end()
        .map_err(|refused| CommandError::new(refused.to_string()))?;
    ctx.request_redraw();
    match times(args) {
        Some(1) => Ok(()),
        more => ctx
            .editor
            .macro_call(more.map(|n| n - 1))
            .map_err(|refused| CommandError::new(refused.to_string())),
    }
}

fn end_and_call(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    if ctx.editor.macro_recording() {
        ctx.editor
            .macro_end()
            .map_err(|refused| CommandError::new(refused.to_string()))?;
    }
    ctx.editor
        .macro_call(times(args))
        .map_err(|refused| CommandError::new(refused.to_string()))?;
    ctx.request_redraw();
    Ok(())
}
