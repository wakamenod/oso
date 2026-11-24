//! Commands that set a buffer's major mode, which names its language, and turn highlighting on
//! and off for every buffer (ADR 0111). Of the modes of ADR 0138, Go, TypeScript, TSX and
//! JavaScript have a grammar (ADR 0147); LaTeX and Swift have none yet. HTML, CSS, JSON and YAML
//! have theirs (ADR 0157).
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
use oso_core::editor::Editor;

/// The command that turns highlighting on and off in every buffer.
pub const FONT_LOCK: &str = "global-font-lock-mode";

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
        name: "go-mode",
        doc: "Set the current buffer's major mode to Go.\n\nIts text is highlighted as Go source. Setting it again in a buffer whose highlighting stopped tries again. In a trusted project its language server follows it once the buffer is visited again or `lsp-start` runs.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: go_mode,
        state: None,
    },
    Command {
        name: "typescript-mode",
        doc: "Set the current buffer's major mode to TypeScript.\n\nIts text is highlighted as TypeScript source. Setting it again in a buffer whose highlighting stopped tries again. In a trusted project its language server follows it once the buffer is visited again or `lsp-start` runs.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: typescript_mode,
        state: None,
    },
    Command {
        name: "tsx-mode",
        doc: "Set the current buffer's major mode to TSX.\n\nIts text is highlighted as TSX source. Setting it again in a buffer whose highlighting stopped tries again. In a trusted project its language server follows it once the buffer is visited again or `lsp-start` runs.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: tsx_mode,
        state: None,
    },
    Command {
        name: "javascript-mode",
        doc: "Set the current buffer's major mode to JavaScript.\n\nIts text is highlighted as JavaScript source. Setting it again in a buffer whose highlighting stopped tries again. In a trusted project its language server follows it once the buffer is visited again or `lsp-start` runs.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: javascript_mode,
        state: None,
    },
    Command {
        name: "html-mode",
        doc: "Set the current buffer's major mode to HTML.\n\nIts text is highlighted as HTML, and the code in its `<script>` and `<style>` elements as JavaScript, JSON or CSS. Setting it again in a buffer whose highlighting stopped tries again.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: html_mode,
        state: None,
    },
    Command {
        name: "css-mode",
        doc: "Set the current buffer's major mode to CSS.\n\nIts text is highlighted as CSS. Setting it again in a buffer whose highlighting stopped tries again.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: css_mode,
        state: None,
    },
    Command {
        name: "json-mode",
        doc: "Set the current buffer's major mode to JSON.\n\nIts text is highlighted as JSON, comments too. Setting it again in a buffer whose highlighting stopped tries again.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: json_mode,
        state: None,
    },
    Command {
        name: "yaml-mode",
        doc: "Set the current buffer's major mode to YAML.\n\nIts text is highlighted as YAML. Setting it again in a buffer whose highlighting stopped tries again.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: yaml_mode,
        state: None,
    },
    Command {
        name: "latex-mode",
        doc: "Set the current buffer's major mode to LaTeX.\n\nIt has no highlighting yet. In a trusted project its language server follows it once the buffer is visited again or `lsp-start` runs.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: latex_mode,
        state: None,
    },
    Command {
        name: "swift-mode",
        doc: "Set the current buffer's major mode to Swift.\n\nIt has no highlighting yet. In a trusted project its language server follows it once the buffer is visited again or `lsp-start` runs.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: swift_mode,
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
        name: FONT_LOCK,
        doc: "Toggle highlighting in every buffer.\n\nA positive prefix argument turns it on, zero or a negative one off.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: global_font_lock_mode,
        state: Some(font_lock_on),
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

fn go_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Go)
}

fn typescript_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::TypeScript)
}

fn tsx_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Tsx)
}

fn javascript_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::JavaScript)
}

fn html_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Html)
}

fn css_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Css)
}

fn json_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Json)
}

fn yaml_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Yaml)
}

fn latex_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Latex)
}

fn swift_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    set_mode(ctx, Mode::Swift)
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

/// Whether buffers are highlighted, for a menu's `[x]` (ADR 0117).
fn font_lock_on(editor: &Editor) -> bool {
    editor.font_lock()
}

#[cfg(test)]
mod tests;
