//! Renaming what is at point through the language server, as Eglot's `eglot-rename`
//! (ADR 0138, step 14): the commands ask the host, which checks the place, reads the new name,
//! shows the edits in `*lsp-rename*` and applies them on `y`.
//!
//! Must not depend on: `app`, the window system or the servers; the host does the asking.
//!
//! - All are `Person`: the agent channel never runs them (Q10).
//! - `lsp-rename` has no key (Q2): `M-x` runs it.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::ServerRequest;

pub const COMMANDS: &[Command] = &[
    Command {
        name: "lsp-rename",
        doc: "Rename what is at point through the language server, as Eglot's `eglot-rename`.\n\nThe server checks the place first if it can, and the new name is read in a prompt that names the old one, which `M-n` gives. Every line the rename changes, in every file, is shown before and after in `*lsp-rename*`: `y` applies it, each buffer's edits one step to undo, and `n`, `q` or `C-g` leaves everything as it was. Files no buffer holds open unshown and stay unsaved, as the buffers it changes do; `C-x s` saves them. A buffer that may hold secrets is never asked about or edited, a rename that would create, rename or delete a file is refused, and so is a new name that a secret was yanked into.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: rename,
        state: None,
    },
    Command {
        name: "lsp-rename-to",
        doc: "Rename what `lsp-rename` checked to the name its prompt read.\n\nThe prompt `lsp-rename` opens runs it with `RET`; it reads nothing else.",
        args: ArgSpec::NewName {
            history: "lsp-rename",
        },
        reach: Reach::Person,
        run: rename_to,
        state: None,
    },
    Command {
        name: "lsp-rename-apply",
        doc: "Apply the rename `*lsp-rename*` shows.\n\n`y` in `*lsp-rename*` runs it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: rename_apply,
        state: None,
    },
    Command {
        name: "lsp-rename-cancel",
        doc: "Drop the rename `*lsp-rename*` shows, or the one waiting for the server, changing nothing.\n\n`n`, `q` and `C-g` in `*lsp-rename*` run it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: rename_cancel,
        state: None,
    },
];

/// The host checks the place and reads the new name.
fn rename(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    if ctx.editor.current().1.is_secret() {
        return Err(CommandError::new(
            "No server for a buffer that may hold secrets",
        ));
    }
    let at = ctx.editor.ask_at();
    ctx.editor.request_server(ServerRequest::Rename(at));
    ctx.request_redraw();
    Ok(())
}

/// The host asks the server to rename to the name read.
fn rename_to(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::refuse_secret_answer(
        ctx,
        "A name holding what may be secret is not sent to the server",
    )?;
    let name = ctx.answer().unwrap_or_default();
    ctx.editor
        .request_server(ServerRequest::RenameTo(name.into()));
    ctx.request_redraw();
    Ok(())
}

fn rename_apply(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.request_server(ServerRequest::ApplyRename);
    ctx.request_redraw();
    Ok(())
}

fn rename_cancel(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.request_server(ServerRequest::CancelRename);
    ctx.request_redraw();
    Ok(())
}
