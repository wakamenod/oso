//! The directory editor's commands (ADR 0134), step 1a: listing a directory, moving between
//! its rows and directories, opening what a row names, refreshing, ordering, hiding details
//! and copying names.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host, which lists directories on the lister's threads. A command whose request
//! opens a file, shows a buffer or writes the clipboard is `Person`: the host carries it out
//! after the command, where the agent channel's guards no longer see it (ADR 0085). What a row names is the
//! host's: a command says which lines it means, and the host reads its row table, never the
//! text.

use std::path::PathBuf;

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::ListingRequest;
use oso_core::selection::Selection;

/// The directory editor's commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "dired",
        doc: "List a directory, as Emacs's dired does, its path read with completion over directories.\n\nThe listing shows every entry but backups ending in `~`, directories first, each with its mode, size, time and name. In it, `RET` opens the row at point, `^` goes up and `g` lists it again. Listing runs nothing and opens no file.",
        args: ArgSpec::Directory {
            prompt: "Dired (directory):",
            history: "file",
        },
        reach: Reach::Person,
        run: dired,
        state: None,
    },
    Command {
        name: "dired-jump",
        doc: "List the directory of the buffer's file, with point on the file.\n\nIn a listing, it goes up to the directory above, as `^` does. A buffer with no file lists the directory oso started in.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: dired_jump,
        state: None,
    },
    Command {
        name: "dired-find-file",
        doc: "In a listing, open what the row at point names: a file in this window, as `C-x C-f` opens it, or a directory in place of the listing.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: find_file,
        state: None,
    },
    Command {
        name: "dired-find-file-other-window",
        doc: "In a listing, open what the row at point names in another window, splitting the frame if it has one window.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: find_file_other_window,
        state: None,
    },
    Command {
        name: "dired-up-directory",
        doc: "In a listing, list the directory above in its place, with point on the one left.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: up_directory,
        state: None,
    },
    Command {
        name: "dired-next-line",
        doc: "In a listing, move down a row, or a count of rows, to the start of its name.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: next_line,
        state: None,
    },
    Command {
        name: "dired-previous-line",
        doc: "In a listing, move up a row, or a count of rows, to the start of its name.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: previous_line,
        state: None,
    },
    Command {
        name: "dired-revert",
        doc: "In a listing, list its directory again (`g`), keeping point on the same name.\n\nA shown listing is also listed again within a second of its directory changing.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: revert,
        state: None,
    },
    Command {
        name: "dired-copy-filename-as-kill",
        doc: "In a listing, copy the names of the rows at the cursors to the kill ring and the clipboard, separated by spaces.\n\nWith a prefix argument of 0, their absolute paths.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: copy_filename_as_kill,
        state: None,
    },
    Command {
        name: "dired-hide-details-mode",
        doc: "In a listing, show its rows with their mode, owner, size and time, or with their names only.\n\nThe listings it moves to keep the choice.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: hide_details,
        state: None,
    },
    Command {
        name: "dired-sort-toggle",
        doc: "In a listing, order its rows by name or by time, newest first, directories first in each.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: sort_toggle,
        state: None,
    },
];

/// The default keys of a listing, from the commands above.
pub const KEYS: &[(&str, &str)] = &[
    ("RET", "dired-find-file"),
    ("f", "dired-find-file"),
    ("o", "dired-find-file-other-window"),
    ("^", "dired-up-directory"),
    ("n", "dired-next-line"),
    ("SPC", "dired-next-line"),
    ("p", "dired-previous-line"),
    ("g", "dired-revert"),
    ("w", "dired-copy-filename-as-kill"),
    ("(", "dired-hide-details-mode"),
    ("s", "dired-sort-toggle"),
    ("q", "quit-window"),
    // Digits are a prefix argument, as in dired, so that `0 w` copies absolute paths.
    ("0", "digit-argument"),
    ("1", "digit-argument"),
    ("2", "digit-argument"),
    ("3", "digit-argument"),
    ("4", "digit-argument"),
    ("5", "digit-argument"),
    ("6", "digit-argument"),
    ("7", "digit-argument"),
    ("8", "digit-argument"),
    ("9", "digit-argument"),
    ("-", "negative-argument"),
];

/// The listing buffer the selected window shows, or why the command means nothing here.
pub(super) fn listing(ctx: &Ctx<'_>) -> Result<oso_core::buffer::BufferId, CommandError> {
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::Directory || ctx.editor.listing(window.buffer()).is_none() {
        return Err(CommandError::new("Not in a directory listing"));
    }
    Ok(window.buffer())
}

/// The buffer lines of the cursors of the selected window, in order, each once.
fn cursor_lines(ctx: &Ctx<'_>) -> Vec<usize> {
    let (window, buffer) = ctx.editor.current();
    let text = buffer.text();
    let mut lines: Vec<usize> = window
        .selections
        .iter()
        .map(|selection| text.offset_to_point(selection.cursor).row)
        .collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

fn dired(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let directory = PathBuf::from(ctx.answer().unwrap_or_default());
    ctx.editor.request_listing(ListingRequest::Open {
        directory,
        point: None,
    });
    ctx.request_redraw();
    Ok(())
}

fn dired_jump(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    if let Ok(buffer) = listing(ctx) {
        ctx.editor.request_listing(ListingRequest::Up { buffer });
        return Ok(());
    }
    let request = match ctx.editor.current().1.path() {
        Some(path) => match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => ListingRequest::Open {
                directory: parent.to_path_buf(),
                point: Some(name.to_os_string()),
            },
            _ => ListingRequest::Open {
                directory: path.to_path_buf(),
                point: None,
            },
        },
        None => ListingRequest::Open {
            directory: ctx.editor.start_directory().to_path_buf(),
            point: None,
        },
    };
    ctx.editor.request_listing(request);
    ctx.request_redraw();
    Ok(())
}

/// Asks the host to open the row at the primary cursor.
fn visit(ctx: &mut Ctx<'_>, other_window: bool) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    let (window, shown) = ctx.editor.current();
    let line = shown
        .text()
        .offset_to_point(window.selections.primary().cursor)
        .row;
    ctx.editor.request_listing(ListingRequest::Visit {
        buffer,
        line,
        other_window,
    });
    ctx.request_redraw();
    Ok(())
}

fn find_file(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    visit(ctx, false)
}

fn find_file_other_window(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    visit(ctx, true)
}

fn up_directory(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    ctx.editor.request_listing(ListingRequest::Up { buffer });
    ctx.request_redraw();
    Ok(())
}

/// Moves every cursor `n` rows, to the start of the row's name, staying on the rows.
pub(super) fn move_rows(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    let Some(record) = ctx.editor.listing(buffer).cloned() else {
        return Ok(());
    };
    let (first, rows) = (record.header_lines, record.rows);
    if rows == 0 {
        return Ok(());
    }
    let last = first + rows - 1;
    let (window, shown) = ctx.editor.current_mut();
    let text = shown.text().clone();
    let update = window.selections.update(|selection: &mut Selection| {
        let row = text.offset_to_point(selection.cursor).row;
        let line = row.saturating_add_signed(n).clamp(first, last);
        selection.cursor = record.name_start(&text, line);
        selection.goal = None;
        Ok::<(), ()>(())
    });
    if update.changed {
        ctx.request_redraw();
    }
    Ok(())
}

fn next_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_rows(ctx, args.count())
}

fn previous_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_rows(ctx, args.count().saturating_neg())
}

fn revert(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    ctx.editor
        .request_listing(ListingRequest::Revert { buffer });
    Ok(())
}

fn copy_filename_as_kill(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    let absolute = args.prefix() == Some(0);
    let lines = cursor_lines(ctx);
    ctx.editor.request_listing(ListingRequest::CopyNames {
        buffer,
        lines,
        absolute,
    });
    Ok(())
}

fn hide_details(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    ctx.editor
        .request_listing(ListingRequest::ToggleDetails { buffer });
    Ok(())
}

fn sort_toggle(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = listing(ctx)?;
    ctx.editor
        .request_listing(ListingRequest::ToggleSort { buffer });
    Ok(())
}

mod marks;

pub use marks::{COMMANDS as MARK_COMMANDS, KEYS as MARK_KEYS};

#[cfg(test)]
mod tests;
