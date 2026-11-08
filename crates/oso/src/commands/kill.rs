//! Commands that kill text into the kill ring and yank it back (ADR 0030, ADR 0058,
//! ADR 0074).
//!
//! Must not depend on: winit, the GPU, the clipboard or `bindings`; a command sees only
//! the host-owned state and does not know its keys.
//!
//! Each works on every cursor of the selected window (ADR 0007): a kill takes one range
//! per cursor through `Editor::kill`, which merges those that overlap, puts the pieces in
//! the ring and deletes them in one edit. A kill right after a kill appends to its entry.
//! `yank` waits for the OS clipboard first, which the dispatcher reads off the UI thread.

use std::ops::Range;

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Prefix, Reach};
use oso_core::kill::{Direction, FromClipboard};
use oso_core::motion::WalkBudget;
use oso_core::rope::Rope;
use oso_core::selection::MarkActive;
use oso_core::word;

use super::budget_left;

/// The most text `kill-line` reads to see whether only blanks follow the cursor, across
/// all its cursors, as one stop of a motion may read (ADR 0058). A rest of the line past
/// what is left counts as not blank.
const MAX_BLANKS: usize = 128 * 1024;

/// The kill and yank commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "kill-region-or-backward-word",
        doc: "Kill the region at every cursor if it shows, or else the previous word.\n\nWithout a region it is `backward-kill-word`, with its count. A kill right after a kill appends to it. In a read-only buffer the text is copied and the command signals.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: kill_region_or_backward_word,
    },
    Command {
        name: "kill-ring-save",
        doc: "Copy the region at every cursor to the kill ring and the clipboard, and hide it.\n\nWith the region hidden, it copies from each cursor to its mark. With no mark, signal \"The mark is not set now, so there is no region\". Right after a kill it appends to it, as in Emacs.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: kill_ring_save,
    },
    Command {
        name: "kill-line",
        doc: "Kill the rest of the line at every cursor, or N lines (backward if N is negative).\n\nAt a line's start, or with only spaces and tabs after the cursor, it takes the line break too (`kill-whole-line`). With N, it takes N lines with their breaks; with 0, back to the line's start. Lines are logical lines. At the buffer's end it signals \"End of buffer\".",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: kill_line,
    },
    Command {
        name: "kill-word",
        doc: "Kill from every cursor to the end of the next word, N times (backward if N is negative).\n\nWords are as in `forward-word`. What a command can kill stops at the buffer's edge, with no signal. A kill right after a kill appends to it.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: kill_word,
    },
    Command {
        name: "backward-kill-word",
        doc: "Kill from every cursor to the start of the previous word, N times (forward if N is negative).\n\nSee `kill-word`. Appended to a kill before it, the text goes first.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: backward_kill_word,
    },
    Command {
        name: "yank",
        doc: "Insert the newest kill at every cursor, or what another app put on the clipboard.\n\nEach cursor gets its own piece of a kill made with as many cursors, and otherwise all the pieces joined with line breaks. The mark goes to the start of the text, and with a plain `C-u` the cursor does. With N, insert the Nth newest kill, and read only the ring. A shown region is replaced. Yanking a kill from a buffer that may hold secrets makes this buffer one too.",
        args: ArgSpec::CountAndClipboard,
        reach: Reach::Person,
        run: yank,
    },
];

/// Kills from every cursor of the selected window across `n` words, backward if `n` is
/// negative. The buffer's edge stops it with no signal, as in Emacs. A cursor the budget
/// runs out on kills nothing, and the command signals after the others have killed.
fn kill_words(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current();
    let display = buffer.display();
    let mut budget = WalkBudget::new();
    let ranges: Vec<Range<usize>> = window
        .selections
        .iter()
        .map(|selection| {
            let to = word::move_words(&display, selection.cursor, n, &mut budget);
            let to = if budget.is_spent() {
                selection.cursor
            } else {
                to
            };
            selection.cursor.min(to)..selection.cursor.max(to)
        })
        .collect();
    kill(ctx, ranges, direction_of(n))?;
    budget_left(budget)
}

fn kill_word(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    kill_words(ctx, args.count())
}

fn backward_kill_word(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    kill_words(ctx, args.count().saturating_neg())
}

fn kill_region_or_backward_word(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let set = &ctx.editor.window().selections;
    if !set.mark_active().is_active() {
        return backward_kill_word(ctx, args);
    }
    let ranges: Vec<Range<usize>> = set.iter().map(|selection| selection.range()).collect();
    kill(ctx, ranges, Direction::Forward)
}

fn kill_ring_save(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let set = &ctx.editor.window().selections;
    if set.iter().any(|selection| selection.mark.is_none()) {
        return Err(CommandError::new(
            "The mark is not set now, so there is no region",
        ));
    }
    let ranges: Vec<Range<usize>> = set.iter().map(|selection| selection.range()).collect();
    let shown = set.mark_active().is_active();
    ctx.editor.kill(ranges, Direction::Forward, true)?;
    if shown {
        ctx.editor
            .current_mut()
            .0
            .selections
            .set_mark_active(MarkActive::Inactive);
        ctx.request_redraw();
    }
    Ok(())
}

fn kill_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let text = ctx.editor.current().1.text();
    let n = args.prefix();
    let mut edge = None;
    let mut blanks = MAX_BLANKS;
    let ranges: Vec<Range<usize>> = ctx
        .editor
        .window()
        .selections
        .iter()
        .map(|selection| {
            line_range(text, selection.cursor, n, &mut blanks).unwrap_or_else(|hit| {
                edge.get_or_insert(hit);
                selection.cursor..selection.cursor
            })
        })
        .collect();
    let direction = direction_of(n.unwrap_or(1));
    kill(ctx, ranges, direction)?;
    edge.map_or(Ok(()), |hit| Err(CommandError::new(hit)))
}

/// What `kill-line` takes at `cursor` with the prefix `n`, or the edge it hit. `blanks` is
/// what the command may still read to look for blanks, across its cursors.
fn line_range(
    text: &Rope,
    cursor: usize,
    n: Option<isize>,
    blanks: &mut usize,
) -> Result<Range<usize>, &'static str> {
    let row = text.offset_to_point(cursor).row;
    match n {
        None => {
            if cursor >= text.len() {
                return Err("End of buffer");
            }
            let end = text.line_end(row);
            let whole = cursor == text.line_start(row) || only_blanks(text, cursor..end, blanks);
            Ok(cursor..if whole { text.line_start(row + 1) } else { end })
        }
        Some(n) if n > 0 => {
            if cursor >= text.len() {
                return Err("End of buffer");
            }
            Ok(cursor..text.line_start(row.saturating_add(n.unsigned_abs())))
        }
        Some(n) => {
            let start = text.line_start(row.saturating_sub(n.unsigned_abs()));
            if n < 0 && cursor == 0 {
                return Err("Beginning of buffer");
            }
            Ok(start..cursor)
        }
    }
}

/// Whether `range` of `text` holds only spaces and tabs, as Emacs's `kill-line` checks,
/// reading no more than `left`, which it spends. A range longer than that counts as not
/// blank.
fn only_blanks(text: &Rope, range: Range<usize>, left: &mut usize) -> bool {
    let Some(rest) = left.checked_sub(range.len()) else {
        return false;
    };
    *left = rest;
    text.chunks_in_range(range)
        .all(|chunk| chunk.bytes().all(|byte| byte == b' ' || byte == b'\t'))
}

/// Which way a kill goes for a count of `n`.
fn direction_of(n: isize) -> Direction {
    if n > 0 {
        Direction::Forward
    } else {
        Direction::Backward
    }
}

/// Kills `ranges`, one per cursor, and asks for a frame if that changed anything.
fn kill(
    ctx: &mut Ctx<'_>,
    ranges: Vec<Range<usize>>,
    direction: Direction,
) -> Result<(), CommandError> {
    let revision = ctx.editor.current().1.revision();
    let result = ctx.editor.kill(ranges, direction, false);
    if ctx.editor.current().1.revision() != revision {
        ctx.request_redraw();
    }
    result?;
    Ok(())
}

fn yank(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let unread = match ctx.clipboard() {
        Some(FromClipboard::Text {
            text,
            concealed,
            fingerprint,
        }) => {
            ctx.editor
                .take_from_clipboard(text.clone(), *concealed, *fingerprint);
            None
        }
        Some(FromClipboard::TooLarge) => Some("Clipboard text too large; yanked the newest kill"),
        Some(FromClipboard::NoAnswer) => Some("Clipboard not read; yanked the newest kill"),
        Some(FromClipboard::Own | FromClipboard::Nothing) | None => None,
    };
    // A plain `C-u` leaves the cursor at the start, and a number N picks the Nth newest
    // entry, going round the ring as Emacs's `current-kill` does.
    let (n, cursor_at_start) = match args.raw() {
        None => (0, false),
        Some(Prefix::Universal(_)) => (0, true),
        Some(prefix) => {
            let len = ctx.editor.kill_ring().len().max(1);
            let n = prefix
                .value()
                .saturating_sub(1)
                .rem_euclid(len.cast_signed());
            (n.unsigned_abs(), false)
        }
    };
    ctx.editor
        .yank(n, cursor_at_start)
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    if let Some(message) = unread {
        ctx.editor.message(message);
    }
    Ok(())
}
