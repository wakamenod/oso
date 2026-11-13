//! Commands that set a buffer's major mode, which names its language, and turn highlighting on
//! and off for every buffer (ADR 0111).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! - A mode command changes the current buffer only, and refuses an internal one, whose mode
//!   decides its keys. Setting the mode it has again gives highlighting that stopped another try.
//! - Nothing here parses: the syntax worker takes the buffer up after the command.
//! - A mode command gives the buffer the mode's indent size and style (ADR 0115).

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The commands for modes and highlighting. They change how a buffer is shown, never its text.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "rust-mode",
        doc: "Set the current buffer's major mode to Rust.\n\nIts text is highlighted as Rust source. Setting it again in a buffer whose highlighting stopped tries again.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: rust_mode,
        state: None,
    },
    Command {
        name: "markdown-mode",
        doc: "Set the current buffer's major mode to Markdown.\n\nIts text is highlighted as Markdown. Setting it again in a buffer whose highlighting stopped tries again.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: markdown_mode,
        state: None,
    },
    Command {
        name: "toml-mode",
        doc: "Set the current buffer's major mode to TOML.\n\nIts text is highlighted as TOML. Setting it again in a buffer whose highlighting stopped tries again.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: toml_mode,
        state: None,
    },
    Command {
        name: "fundamental-mode",
        doc: "Set the current buffer's major mode to Fundamental.\n\nIts text is not highlighted.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: fundamental_mode,
        state: None,
    },
    Command {
        name: "global-font-lock-mode",
        doc: "Toggle highlighting in every buffer.\n\nA positive prefix argument turns it on, zero or a negative one off.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: global_font_lock_mode,
        state: None,
    },
];

fn rust_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Rust)
}

fn markdown_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Markdown)
}

fn toml_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Toml)
}

fn fundamental_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Fundamental)
}

/// Sets the current buffer's mode, and says so; the modeline names it.
fn set_mode(ctx: &mut Ctx<'_>, mode: Mode) -> Result<(), CommandError> {
    let id = ctx.editor.window().buffer();
    // The mode's indent size and style under what EditorConfig gives the file (ADR 0115); the
    // tab width stays the one it is shown with.
    if !ctx.editor.set_buffer_mode(id, mode) {
        return Err(CommandError::new("This buffer's mode cannot change"));
    }
    ctx.editor.message(format!("{} mode", mode.name()));
    ctx.request_redraw();
    Ok(())
}

/// Turns highlighting on or off as Emacs's minor modes read their argument: none toggles, a
/// positive one turns it on and zero or a negative one off. Says what it is now.
fn global_font_lock_mode(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let was = ctx.editor.font_lock();
    let now = super::minor_mode_on(args, was);
    ctx.editor.set_font_lock(now);
    ctx.editor
        .message(super::minor_mode_said("Global Font-Lock", now));
    if now != was {
        ctx.request_redraw();
    }
    Ok(())
}

#[cfg(test)]
mod tests;
