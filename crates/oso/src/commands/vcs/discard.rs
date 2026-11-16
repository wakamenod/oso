//! Source control's commands for discarding (ADR 0127, step 1c): `k` in a status, the list of
//! what discards kept, and writing a copy back.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command asks the host, which asks the
//! question, keeps the copies and runs git. Each is `Person`: it changes the work tree, or
//! shows a list whose keys are the person's (ADR 0085).

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::GitRequest;

macro_rules! command {
    ($name:literal, $doc:literal, $run:ident) => {
        Command {
            name: $name,
            doc: $doc,
            args: ArgSpec::None,
            reach: Reach::Person,
            run: $run,
            state: None,
        }
    };
}

/// The commands of discarding.
pub const COMMANDS: &[Command] = &[
    command!(
        "git-discard",
        "In a git status, discard what each cursor is on (`k`): a section's files, a file, a hunk, or the lines a region holds of one, after a question.\n\nUnstaged changes go from the work tree, staged ones from the index and the work tree, and untracked files are deleted. Each file is kept first as one of git's objects for two weeks, but one that may hold secrets; `M-x git-discarded` lists them and writes one back.",
        discard
    ),
    command!(
        "git-discarded",
        "Show what discards kept in the project's repository, newest first, in `*git discarded: NAME*`; `RET` there writes one back, after a question.",
        show_discarded
    ),
    command!(
        "git-discarded-refresh",
        "In the list of what discards kept, read it again (`g`).",
        refresh_discarded
    ),
    command!(
        "git-restore-discarded",
        "In the list of what discards kept, write the copy at point back to its file (`RET`), after a question; what the file holds then is kept too.",
        restore_discarded
    ),
];

/// The keys of the list of what discards kept.
pub const LIST_KEYS: &[(&str, &str)] = &[
    ("RET", "git-restore-discarded"),
    ("g", "git-discarded-refresh"),
    ("q", "quit-window"),
];

fn discard(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = super::status(ctx)?;
    let lines = super::cursor_lines(ctx);
    let confirmed = ctx.confirmed();
    ctx.editor.request_git(GitRequest::Discard {
        buffer,
        lines,
        confirmed,
    });
    super::deactivate_mark(ctx);
    ctx.request_redraw();
    Ok(())
}

fn show_discarded(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    // The tiled window's, as `M-x` runs it with the minibuffer selected.
    let buffer = ctx.editor.tiled().0.buffer();
    let in_list =
        (ctx.editor.buffer(buffer)).is_some_and(|shown| shown.mode() == Mode::GitDiscarded);
    if !in_list && super::status_project(ctx.editor).is_none() {
        return Err(CommandError::new("Not in a project"));
    }
    ctx.editor.request_git(GitRequest::ShowDiscarded { buffer });
    ctx.request_redraw();
    Ok(())
}

fn refresh_discarded(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::GitDiscarded {
        return Err(CommandError::new("Not in the list of what discards kept"));
    }
    let buffer = window.buffer();
    ctx.editor.request_git(GitRequest::ShowDiscarded { buffer });
    ctx.request_redraw();
    Ok(())
}

fn restore_discarded(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::GitDiscarded {
        return Err(CommandError::new("Not in the list of what discards kept"));
    }
    let line = buffer
        .text()
        .offset_to_point(window.selections.primary().cursor)
        .row;
    let buffer = window.buffer();
    let confirmed = ctx.confirmed();
    ctx.editor.request_git(GitRequest::RestoreDiscarded {
        buffer,
        line,
        confirmed,
    });
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests;
