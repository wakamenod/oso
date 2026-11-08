//! Commands that change text: typing, deleting and undoing (ADR 0030, ADR 0051,
//! ADR 0056, ADR 0058).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! Each makes one edit with one replacement per cursor of the selected window
//! (ADR 0007), through `Editor::apply_edits`, which moves the cursors and refuses
//! a read-only buffer. The dispatcher opens and closes the undo group around a
//! command, so one command is one undo step across all cursors.

use std::borrow::Cow;

use oso_core::buffer::{EditError, Replacement};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::display_map::{CellKind, DisplayPoint, DisplaySnapshot, TabWidth};
use oso_core::motion::{self, Edge, WalkBudget};
use oso_core::word;

use super::budget_left;

/// The most text one command inserts across all cursors, so that a count cannot
/// run the process out of memory, which aborts it with no emergency save
/// (ADR 0051). It takes about 40 ms to insert, which is no freeze.
const MAX_INSERT: usize = 16 * 1024 * 1024;

/// The most undo steps one command crosses. A step of joined typing has 20 small
/// edits at about 9 µs each, so this takes about 50 ms, and more for text it puts
/// back. The history is bounded in bytes, but it can hold many small steps.
const MAX_CROSSINGS: usize = 256;

/// The editing commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "self-insert-command",
        doc: "Insert the typed character at every cursor, N times.\n\nSingle characters typed in a row undo together, up to 20 of them. A count over 1, text of several characters, or an input method's commit, is an undo step of its own. A negative N signals an error.",
        args: ArgSpec::Key,
        run: self_insert_command,
    },
    Command {
        name: "insert-full-width-space",
        doc: "Insert a full-width space (U+3000) at every cursor, N times.\n\n`SPC` inserts a half-width one, also with a Japanese input source. It undoes with the typing around it, as one typed character does. A negative N signals an error.",
        args: ArgSpec::Count,
        run: insert_full_width_space,
    },
    Command {
        name: "newline",
        doc: "Insert a line break at every cursor, N times.\n\nThe line break is the one the file uses, so `\\r\\n` in a CRLF file. Each is an undo step of its own. A negative N signals an error.",
        args: ArgSpec::Count,
        run: newline,
    },
    Command {
        name: "delete-backward-char",
        doc: "Delete the N characters before every cursor (after it if N is negative).\n\nCharacters are counted as in `forward-char`. A cursor with fewer than N characters before it deletes nothing and signals \"Beginning of buffer\"; other cursors still delete. So does one a count would read too much for, signaling \"Count too large\".",
        args: ArgSpec::Count,
        run: delete_backward_char,
    },
    Command {
        name: "delete-char",
        doc: "Delete the N characters after every cursor (before it if N is negative).\n\nCharacters are counted as in `forward-char`. A cursor with fewer than N characters after it deletes nothing and signals \"End of buffer\"; other cursors still delete. So does one a count would read too much for, signaling \"Count too large\".",
        args: ArgSpec::Count,
        run: delete_char,
    },
    Command {
        name: "kill-word",
        doc: "Delete from every cursor to the end of the next word, N times (backward if N is negative).\n\nWords are as in `forward-word`. What a command can delete stops at the buffer's edge, with no signal. Until the kill ring arrives, the text does not go to it.",
        args: ArgSpec::Count,
        run: kill_word,
    },
    Command {
        name: "backward-kill-word",
        doc: "Delete from every cursor to the start of the previous word, N times (forward if N is negative).\n\nSee `kill-word`.",
        args: ArgSpec::Count,
        run: backward_kill_word,
    },
    Command {
        name: "tab-to-tab-stop",
        doc: "Insert spaces at every cursor, up to the next tab stop.\n\nTab stops are every `tab-width` cells. Each cursor goes to its own next stop. Indenting by mode comes with modes.",
        args: ArgSpec::None,
        run: tab_to_tab_stop,
    },
    Command {
        name: "undo",
        doc: "Undo the last N changes to the buffer (redo them if N is negative).\n\nThe cursors go back to where they were. With nothing more to undo, signal \"No further undo information\". After 256 changes, stop and signal \"Count too large\".",
        args: ArgSpec::Count,
        run: undo,
    },
    Command {
        name: "undo-redo",
        doc: "Redo the last N undone changes (undo if N is negative).\n\nAny edit after an undo discards what could be redone (ADR 0030). With nothing more to redo, signal \"No further redo information\".",
        args: ArgSpec::Count,
        run: undo_redo,
    },
];

/// Inserts `text` at every cursor of the selected window, `count` times. Asks
/// for a redraw if that changes anything.
fn insert_at_cursors(ctx: &mut Ctx<'_>, text: &str, count: isize) -> Result<(), CommandError> {
    let times = usize::try_from(count)
        .map_err(|_| CommandError::new(format!("Negative repetition argument {count}")))?;
    let window = ctx.editor.window();
    let (buffer, cursors) = (window.buffer(), window.selections.len());
    let too_much = text
        .len()
        .checked_mul(times)
        .and_then(|bytes| bytes.checked_mul(cursors))
        .is_none_or(|bytes| bytes > MAX_INSERT);
    if too_much {
        return Err(CommandError::new("Too much text to insert"));
    }
    if times == 0 {
        return Ok(());
    }
    let text = if times == 1 {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.repeat(times))
    };
    let edits: Vec<Replacement<'_>> = window
        .selections
        .iter()
        .map(|selection| Replacement {
            range: selection.cursor..selection.cursor,
            text: &text,
        })
        .collect();
    ctx.editor.apply_edits(buffer, &edits)?;
    ctx.request_redraw();
    Ok(())
}

fn self_insert_command(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let input = ctx.input();
    if input.is_empty() {
        return Err(CommandError::new("No text to insert"));
    }
    type_text(ctx, input, args.count())
}

/// Inserts `text` as typing does: a single character with a count under 2 asks to
/// join the typing before it, as Emacs does. Text of several characters stays out, or
/// a run of them would undo as one step.
fn type_text(ctx: &mut Ctx<'_>, text: &str, count: isize) -> Result<(), CommandError> {
    if count < 2 && text.chars().nth(1).is_none() {
        ctx.editor.allow_undo_join();
    }
    insert_at_cursors(ctx, text, count)
}

/// The space a Japanese input method types for `SPC`, which winit cannot deliver
/// (ADR 0069).
const FULL_WIDTH_SPACE: &str = "\u{3000}";

fn insert_full_width_space(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    type_text(ctx, FULL_WIDTH_SPACE, args.count())
}

fn newline(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let ending = ctx.editor.current().1.file_info().line_ending.text();
    insert_at_cursors(ctx, ending, args.count())
}

/// Deletes `n` display clusters at every cursor of the selected window, after
/// it, or before it if `n` is negative. A cursor that cannot pass them all
/// deletes nothing, as Emacs's `delete-char` does, and the first such cursor's
/// edge is reported after the others have deleted.
fn delete_clusters(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current();
    if let Some(reason) = buffer.read_only() {
        return Err(EditError::ReadOnly(reason).into());
    }
    let display = buffer.display();
    let len = motion::buffer_end(&display);
    let mut budget = WalkBudget::new();
    let mut edits = Vec::with_capacity(window.selections.len());
    let mut edge = None;
    for selection in window.selections.iter() {
        let mut moved = *selection;
        // A cluster takes at least a byte, so a count past the buffer's edge
        // needs no walk there.
        let room = if n < 0 {
            selection.cursor
        } else {
            len - selection.cursor
        };
        let walked = if n.unsigned_abs() > room {
            Err(if n < 0 { Edge::Start } else { Edge::End })
        } else {
            motion::move_clusters(&display, &mut moved, n, &mut budget)
        };
        match walked {
            // Stopped short: it could not go all the way.
            Ok(()) if budget.is_spent() => {}
            Ok(()) if moved.cursor != selection.cursor => edits.push(Replacement {
                range: selection.cursor.min(moved.cursor)..selection.cursor.max(moved.cursor),
                text: "",
            }),
            Ok(()) => {}
            Err(hit) => {
                edge.get_or_insert(hit);
            }
        }
    }
    if !edits.is_empty() {
        let buffer = window.buffer();
        ctx.editor.apply_edits(buffer, &edits)?;
        ctx.request_redraw();
    }
    match edge {
        Some(edge) => Err(CommandError::new(edge.message())),
        None => budget_left(budget),
    }
}

fn delete_backward_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    delete_clusters(ctx, args.count().saturating_neg())
}

fn delete_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    delete_clusters(ctx, args.count())
}

/// Deletes from every cursor of the selected window across `n` words, backward if
/// `n` is negative, in one edit. The buffer's edge stops it with no signal, as in
/// Emacs. A cursor the budget runs out on deletes nothing, and the command signals
/// after the others have deleted. This is where the kill ring comes in (M2-14).
fn delete_words(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current();
    if let Some(reason) = buffer.read_only() {
        return Err(EditError::ReadOnly(reason).into());
    }
    let display = buffer.display();
    let mut budget = WalkBudget::new();
    let mut edits = Vec::with_capacity(window.selections.len());
    for selection in window.selections.iter() {
        let to = word::move_words(&display, selection.cursor, n, &mut budget);
        if !budget.is_spent() && to != selection.cursor {
            edits.push(Replacement {
                range: selection.cursor.min(to)..selection.cursor.max(to),
                text: "",
            });
        }
    }
    if !edits.is_empty() {
        let buffer = window.buffer();
        ctx.editor.apply_edits(buffer, &edits)?;
        ctx.request_redraw();
    }
    budget_left(budget)
}

fn kill_word(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    delete_words(ctx, args.count())
}

fn backward_kill_word(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    delete_words(ctx, args.count().saturating_neg())
}

fn tab_to_tab_stop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current();
    let display = buffer.display();
    let tab_width = buffer.tab_width();
    let spaces = " ".repeat(tab_width.get() as usize);
    let mut edits = Vec::with_capacity(window.selections.len());
    // The cursor before this one, if it is on the same row, and the column it ends at.
    let mut earlier: Option<(DisplayPoint, usize)> = None;
    for selection in window.selections.iter() {
        let point = display.offset_to_display_point(selection.cursor);
        let column = match earlier {
            Some((from, column)) if from.row == point.row => {
                column_after(&display, tab_width, from, column, selection.cursor)
            }
            _ => point.column,
        };
        let stop = tab_width.next_stop(column);
        edits.push(Replacement {
            range: selection.cursor..selection.cursor,
            text: &spaces[..stop - column],
        });
        earlier = Some((point, stop));
    }
    let buffer = window.buffer();
    ctx.editor.apply_edits(buffer, &edits)?;
    ctx.request_redraw();
    Ok(())
}

/// The column of the offset `to` on the row of `from`, once what is at `from` starts
/// at `column`, after an earlier insertion. What lies between keeps its cells, except
/// that a tab reaches the next stop from where it now starts.
fn column_after(
    display: &DisplaySnapshot,
    tab_width: TabWidth,
    from: DisplayPoint,
    mut column: usize,
    to: usize,
) -> usize {
    for cluster in display.clusters(from) {
        if cluster.offset >= to {
            break;
        }
        column = match cluster.kind {
            CellKind::Tab => tab_width.next_stop(column),
            _ => column + cluster.cells,
        };
    }
    column
}

/// Undoes `n` steps, or redoes them if `n` is negative, stopping at the first
/// that fails, or signaling after [`MAX_CROSSINGS`].
fn cross(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    for _ in 0..n.unsigned_abs().min(MAX_CROSSINGS) {
        if n < 0 {
            ctx.editor.redo()?;
        } else {
            ctx.editor.undo()?;
        }
        ctx.request_redraw();
    }
    if n.unsigned_abs() > MAX_CROSSINGS {
        return Err(CommandError::new(motion::TOO_MUCH));
    }
    Ok(())
}

fn undo(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    cross(ctx, args.count())
}

fn undo_redo(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    cross(ctx, args.count().saturating_neg())
}
