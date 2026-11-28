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
use oso_core::display_map::DisplaySnapshot;
use oso_core::kill::{Direction, FromClipboard};
use oso_core::motion::{self, TOO_MUCH, WalkBudget};
use oso_core::rope::Rope;
use oso_core::selection::MarkActive;
use oso_core::sum_tree::Bias;
use oso_core::word;

use super::budget_left;

/// The most text `kill-line` reads to see whether only blanks follow the cursor, across
/// all its cursors, as one stop of a motion may read (ADR 0058). A rest of the line past
/// what is left counts as not blank.
const MAX_BLANKS: usize = 128 * 1024;

/// The most text the zaps read to find their character, across all their cursors, as a
/// motion's budget (ADR 0058, ADR 0190). A cursor that would read past it kills nothing.
const MAX_ZAP: usize = 4 * 1024 * 1024;

/// The most text a backward zap reads at a time, to look in it from its end.
const ZAP_WINDOW: usize = 64 * 1024;

/// The kill and yank commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "zap-to-char",
        doc: "Kill from every cursor up to and through the Nth next CHAR, read in a prompt (backward if N is negative).\n\nA lowercase CHAR matches either case, an uppercase one only itself. What it kills ends at a display cluster's edge, so a letter goes with its combining marks. A cursor that finds no CHAR kills nothing, and the command signals \"Search failed\" after the others have killed. A kill right after a kill appends to it. The agent channel gives CHAR as text.",
        args: ArgSpec::CountAndChar {
            prompt: "Zap to char:",
        },
        reach: Reach::Change,
        run: zap_to_char,
        state: None,
    },
    Command {
        name: "zap-up-to-char",
        doc: "Kill from every cursor up to, but not through, the Nth next CHAR, read in a prompt (backward if N is negative).\n\nA CHAR right at the cursor is passed over first, so that repeating it goes on. Otherwise as `zap-to-char`.",
        args: ArgSpec::CountAndChar {
            prompt: "Zap up to char:",
        },
        reach: Reach::Change,
        run: zap_up_to_char,
        state: None,
    },
    Command {
        name: "kill-region-or-backward-word",
        doc: "Kill the region at every cursor if it shows, or else the previous word.\n\nWithout a region it is `backward-kill-word`, with its count. A kill right after a kill appends to it. In a read-only buffer the text is copied and the command signals.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: kill_region_or_backward_word,
        state: None,
    },
    Command {
        name: "kill-ring-save",
        doc: "Copy the region at every cursor to the kill ring and the clipboard, and hide it.\n\nWith the region hidden, it copies from each cursor to its mark. With no mark, signal \"The mark is not set now, so there is no region\". Right after a kill it appends to it, as in Emacs.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: kill_ring_save,
        state: None,
    },
    Command {
        name: "kill-line",
        doc: "Kill the rest of the line at every cursor, or N lines (backward if N is negative).\n\nAt a line's start, or with only spaces and tabs after the cursor, it takes the line break too (`kill-whole-line`). With N, it takes N lines with their breaks; with 0, back to the line's start. Lines are logical lines. At the buffer's end it signals \"End of buffer\".",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: kill_line,
        state: None,
    },
    Command {
        name: "kill-word",
        doc: "Kill from every cursor to the end of the next word, N times (backward if N is negative).\n\nWords are as in `forward-word`. What a command can kill stops at the buffer's edge, with no signal. A kill right after a kill appends to it.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: kill_word,
        state: None,
    },
    Command {
        name: "backward-kill-word",
        doc: "Kill from every cursor to the start of the previous word, N times (forward if N is negative).\n\nSee `kill-word`. Appended to a kill before it, the text goes first.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: backward_kill_word,
        state: None,
    },
    Command {
        name: "yank",
        doc: "Insert the newest kill at every cursor, or what another app put on the clipboard.\n\nEach cursor gets its own piece of a kill made with as many cursors, and otherwise all the pieces joined with line breaks. The mark goes to the start of the text, and with a plain `C-u` the cursor does. With N, insert the Nth newest kill, and read only the ring. A shown region is replaced. Yanking a kill from a buffer that may hold secrets makes this buffer one too.",
        args: ArgSpec::CountAndClipboard,
        reach: Reach::Person,
        run: yank,
        state: None,
    },
    Command {
        name: "consult-yank-pop",
        doc: "Yank a kill picked from the kill ring, with its text beside the list.\n\nThe clipboard is read first, as `yank` reads it. Right after a yank, the kill picked takes the place of what it inserted. The kill picked goes to the front of the ring. A kill from a buffer that may hold secrets is listed with no text.",
        args: ArgSpec::KillEntry {
            prompt: "Yank from kill ring:",
        },
        reach: Reach::Person,
        run: consult_yank_pop,
        state: None,
    },
    Command {
        name: "yank-pop",
        doc: "Right after a yank, put the next older kill in place of what it inserted.\n\nRepeated, go further back, round to the newest. Kills from a buffer that may hold secrets are passed over.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: yank_pop,
        state: None,
    },
];

fn consult_yank_pop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor
        .yank_picked()
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

fn yank_pop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor
        .yank_pop()
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

/// Kills from every cursor of the selected window across `n` words, backward if `n` is
/// negative. The buffer's edge stops it with no signal, as in Emacs. A cursor the budget
/// runs out on kills nothing, and the command signals after the others have killed.
fn kill_words(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current();
    let display = buffer.display();
    let mut budget = WalkBudget::for_cursors(window.selections.len());
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

fn zap_to_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    zap(ctx, args, false)
}

fn zap_up_to_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    zap(ctx, args, true)
}

/// Why a cursor's zap killed nothing.
#[derive(Clone, Copy)]
enum Unzapped {
    /// There is no such character there.
    NotFound,
    /// Finding it would read more than [`MAX_ZAP`].
    TooFar,
    /// `zap-up-to-char` could not pass the cluster at the cursor.
    Edge(&'static str),
}

/// Kills at every cursor up to the `N`th character read, through it unless `up_to`. A cursor
/// that finds none kills nothing, and the first such cursor's reason signals after the others
/// have killed. Its character stays out of the signal if a secret was yanked into the prompt
/// that read it.
fn zap(ctx: &mut Ctx<'_>, args: Args, up_to: bool) -> Result<(), CommandError> {
    let Some(target) = ctx.char_answer() else {
        return Ok(());
    };
    // Where the kill ends would tell the character, and the kill would hold it, unmarked.
    let secret = ctx.answer_secret();
    super::replace::refuse_secret(ctx.editor, secret)?;
    let n = args.count();
    let (window, buffer) = ctx.editor.current();
    let display = buffer.display();
    let text = buffer.text();
    let mut left = MAX_ZAP;
    let mut failed = None;
    let ranges: Vec<Range<usize>> = window
        .selections
        .iter()
        .map(|selection| {
            let cursor = selection.cursor;
            zap_range(text, &display, cursor, (target, n, up_to), &mut left).unwrap_or_else(|why| {
                failed.get_or_insert(why);
                cursor..cursor
            })
        })
        .collect();
    kill(ctx, ranges, direction_of(n))?;
    match failed {
        None => Ok(()),
        Some(Unzapped::NotFound) if secret => Err(CommandError::new("Search failed")),
        Some(Unzapped::NotFound) => {
            let shown = target.to_string();
            Err(CommandError::new(format!("Search failed: {shown:?}")))
        }
        Some(Unzapped::TooFar) => Err(CommandError::new(TOO_MUCH)),
        Some(Unzapped::Edge(edge)) => Err(CommandError::new(edge)),
    }
}

/// What a zap from `cursor` kills, for the character `target`, `n` of them, and through the
/// last unless `up_to`, reading no more than `left`, which it spends. Its ends are display
/// clusters' edges. As Emacs's `zap-up-to-char`, `up_to` passes the cluster at the cursor
/// first, and stops at the start of the last match forward, or after it backward.
fn zap_range(
    text: &Rope,
    display: &DisplaySnapshot,
    cursor: usize,
    (target, n, up_to): (char, isize, bool),
    left: &mut usize,
) -> Result<Range<usize>, Unzapped> {
    let count = n.unsigned_abs();
    if count == 0 {
        return Ok(cursor..cursor);
    }
    let matches = char_matcher(target);
    if n > 0 {
        let from = if up_to {
            motion::next_cluster(display, cursor).ok_or(Unzapped::Edge("End of buffer"))?
        } else {
            cursor
        };
        let (found, len) = find_forward(text, from, &matches, count, left)?;
        let end = if up_to {
            motion::cluster_start(display, found)
        } else {
            motion::cluster_end(display, found + len)
        };
        Ok(cursor..end.max(cursor))
    } else {
        let from = if up_to {
            motion::prev_cluster(display, cursor).ok_or(Unzapped::Edge("Beginning of buffer"))?
        } else {
            cursor
        };
        let (found, len) = find_backward(text, from, &matches, count, left)?;
        let start = if up_to {
            motion::cluster_end(display, found + len)
        } else {
            motion::cluster_start(display, found)
        };
        Ok(start.min(cursor)..cursor)
    }
}

/// Whether a character of the text matches `target`, as Emacs's `zap-to-char` searches with
/// `case-fold-search` on: a lowercase `target` matches the uppercase letters it is the
/// lowercase of, too, and an uppercase one only itself.
fn char_matcher(target: char) -> impl Fn(char) -> bool {
    let folds = !target.is_uppercase();
    move |c| {
        c == target
            || (folds && c.is_uppercase() && {
                let mut lower = c.to_lowercase();
                lower.next() == Some(target) && lower.next().is_none()
            })
    }
}

/// The start and the length of the `count`th match at or after `from`, reading no more than
/// `left`. A folded match may be longer or shorter than the character read.
fn find_forward(
    text: &Rope,
    from: usize,
    matches: &impl Fn(char) -> bool,
    mut count: usize,
    left: &mut usize,
) -> Result<(usize, usize), Unzapped> {
    let mut at = from;
    for chunk in text.chunks_in_range(from..text.len()) {
        for (index, c) in chunk.char_indices() {
            *left = left.checked_sub(c.len_utf8()).ok_or(Unzapped::TooFar)?;
            if matches(c) {
                count -= 1;
                if count == 0 {
                    return Ok((at + index, c.len_utf8()));
                }
            }
        }
        at += chunk.len();
    }
    Err(Unzapped::NotFound)
}

/// The start and the length of the `count`th match that ends at or before `from`, going back,
/// reading no more than `left`. It reads [`ZAP_WINDOW`] at a time, from its start, and looks in
/// it from its end.
fn find_backward(
    text: &Rope,
    from: usize,
    matches: &impl Fn(char) -> bool,
    mut count: usize,
    left: &mut usize,
) -> Result<(usize, usize), Unzapped> {
    let mut end = from;
    let mut found = Vec::new();
    while end > 0 {
        if *left == 0 {
            return Err(Unzapped::TooFar);
        }
        let start = text.clip_offset(end.saturating_sub(ZAP_WINDOW.min(*left)), Bias::Left);
        let start = if start == end {
            // A character longer than what is left.
            return Err(Unzapped::TooFar);
        } else {
            start
        };
        *left = left.saturating_sub(end - start);
        found.clear();
        let mut at = start;
        for chunk in text.chunks_in_range(start..end) {
            found.extend(
                chunk
                    .char_indices()
                    .filter(|(_, c)| matches(*c))
                    .map(|(index, c)| (at + index, c.len_utf8())),
            );
            at += chunk.len();
        }
        if let Some(found) = found.iter().rev().nth(count.saturating_sub(1)) {
            return Ok(*found);
        }
        count -= found.len();
        end = start;
    }
    Err(Unzapped::NotFound)
}
