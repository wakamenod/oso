//! A listing's file operations (ADR 0134, step 1b-2): `C` copies, `R` renames or moves, `D`
//! deletes and `+` makes a directory, and the commands the questions before them run on `y`.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command says which lines it means and
//! where they go, and the host runs the operation on its thread. Each changes files the agent
//! channel did not write and is carried out after the command, where the channel's guards no
//! longer see it, so each is `Person`.

use std::path::PathBuf;

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::ListingRequest;

use super::listing;
use super::marks::marked_lines;

/// The operation commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "dired-do-copy",
        doc: "In a listing, copy the marked rows, or the row at point, to a directory read in the prompt, or one row to a new name.\n\nThe prompt starts at the directory of the next window's listing, or with `C-u` this one's. A copy keeps the mode and times, and on macOS the extended attributes; a link is copied as a link, and a directory with all it holds. What is there already is written over only after a question that names it.",
        args: ArgSpec::Target {
            prompt: "Copy to:",
            history: "file",
        },
        reach: Reach::Person,
        run: copy,
        state: None,
    },
    Command {
        name: "dired-do-rename",
        doc: "In a listing, move the marked rows, or the row at point, to a directory read in the prompt, or rename one row.\n\nThe prompt starts as `dired-do-copy`'s. Across file systems it copies, then deletes the source. A file whose buffer has unsaved edits is not moved; a buffer visiting a moved file follows it.",
        args: ArgSpec::Target {
            prompt: "Move to:",
            history: "file",
        },
        reach: Reach::Person,
        run: rename,
        state: None,
    },
    Command {
        name: "dired-do-delete",
        doc: "In a listing, delete the marked rows, or the row at point or the next rows with a count, after a question that names them.\n\nIt deletes for good, as Emacs does by default: a directory goes with all it holds, never following a link and never going into another file system. A file whose buffer has unsaved edits is not deleted.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: delete,
        state: None,
    },
    Command {
        name: "dired-create-directory",
        doc: "Make a directory, read in the prompt from this listing's directory, with the directories above it that are missing.",
        args: ArgSpec::File {
            prompt: "Create directory:",
            history: "file",
        },
        reach: Reach::Person,
        run: create_directory,
        state: None,
    },
    Command {
        name: "dired-do-delete-confirmed",
        doc: "Delete what `dired-do-delete` asked about, as its question's `y` does.\n\nThe listing must still be the one asked about; the operation checks the directory again before it deletes anything. Only the question runs it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: delete_confirmed,
        state: None,
    },
    Command {
        name: "dired-do-overwrite-confirmed",
        doc: "Copy or move what a question asked about, writing over what is there, as its `y` does.\n\nIt writes over only the targets the question named: a different set found then asks again. Only the question runs it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: overwrite_confirmed,
        state: None,
    },
];

/// The listing's keys for its operations.
pub const KEYS: &[(&str, &str)] = &[
    ("C", "dired-do-copy"),
    ("R", "dired-do-rename"),
    ("D", "dired-do-delete"),
    ("+", "dired-create-directory"),
];

/// The path the prompt read.
fn answer(ctx: &Ctx<'_>) -> PathBuf {
    PathBuf::from(ctx.answer().unwrap_or_default())
}

fn copy(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    let (lines, _) = marked_lines(ctx, 1);
    let target = answer(ctx);
    ctx.editor.request_listing(ListingRequest::Copy {
        buffer,
        lines,
        target,
    });
    ctx.request_redraw();
    Ok(())
}

fn rename(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    let (lines, _) = marked_lines(ctx, 1);
    let target = answer(ctx);
    ctx.editor.request_listing(ListingRequest::Rename {
        buffer,
        lines,
        target,
    });
    ctx.request_redraw();
    Ok(())
}

fn delete(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    let (lines, _) = marked_lines(ctx, args.count());
    ctx.editor
        .request_listing(ListingRequest::Delete { buffer, lines });
    ctx.request_redraw();
    Ok(())
}

fn create_directory(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    let path = answer(ctx);
    ctx.editor
        .request_listing(ListingRequest::Mkdir { buffer, path });
    ctx.request_redraw();
    Ok(())
}

fn delete_confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.request_listing(ListingRequest::DeleteConfirmed);
    ctx.request_redraw();
    Ok(())
}

fn overwrite_confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor
        .request_listing(ListingRequest::OverwriteConfirmed);
    ctx.request_redraw();
    Ok(())
}
