//! Commands that move, kill, mark and transpose by balanced expressions, as Emacs's
//! `forward-sexp` and its kin (ADR 0190, step 2).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned
//! state and does not know its keys.
//!
//! What a sexp is, `oso_core::sexp` reads from the buffer's display snapshot, its highlights
//! where it has them, and its mode's table. Every command works at each cursor of the
//! selected window on its own (ADR 0007), in order with the budget they share, and the first
//! error shows after all have done what they could. A cursor whose scan fails stays, and
//! kills, marks and swaps nothing. The motions keep a shown region and select with Shift, as
//! Emacs's `^p` commands do (ADR 0071).

use std::ops::Range;

use oso_core::buffer::{Buffer, Replacement};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::display_map::DisplaySnapshot;
use oso_core::motion::WalkBudget;
use oso_core::selection::{MarkActive, Selection};
use oso_core::sexp::{self, ScanError, Syntax, Table};

use super::MAX_INSERT;
use super::kill::{direction_of, kill};
use super::mark::{NO_MARK, shift_select};

/// The expression commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "forward-sexp",
        doc: "Move point over N balanced expressions (backward if N is negative).\n\nAn expression is a string, a bracket group with what it holds, or a run of letters, digits and `_`, with the mode's own symbol characters; space, punctuation and comments between them are passed. Strings and comments are as highlighting says, or else as each line reads in the mode. Past the last expression, point goes to the buffer's edge. A closer first signals \"Containing expression ends prematurely\", and a bracket with no match \"Unbalanced parentheses\"; a scan that would read too much signals \"Count too large\". Then the cursor stays.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: forward_sexp,
        state: None,
    },
    Command {
        name: "backward-sexp",
        doc: "Move point back over N balanced expressions (forward if N is negative).\n\nSee `forward-sexp`.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: backward_sexp,
        state: None,
    },
    Command {
        name: "forward-list",
        doc: "Move point over N bracket groups (backward if N is negative), passing what is not one.\n\nPast the last group, point goes to the buffer's edge. The errors are `forward-sexp`'s.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: forward_list,
        state: None,
    },
    Command {
        name: "backward-list",
        doc: "Move point back over N bracket groups (forward if N is negative), passing what is not one.\n\nSee `forward-list`.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: backward_list,
        state: None,
    },
    Command {
        name: "backward-up-list",
        doc: "Move point back out of N levels of brackets, to the opener (past the closer if N is negative).\n\nInside a string or a comment, the first level is going to its start. With no bracket around point, signal \"Unbalanced parentheses\".",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: backward_up_list,
        state: None,
    },
    Command {
        name: "up-list",
        doc: "Move point out of N levels of brackets, past the closer (to the opener if N is negative).\n\nInside a string or a comment, the first level is going past its end. See `backward-up-list`.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: up_list,
        state: None,
    },
    Command {
        name: "down-list",
        doc: "Move point into N levels of brackets, past the next opener.\n\nWith a negative N, go backward into the group before point, and stop before its closer. With no group to go into, signal \"Unbalanced parentheses\", or \"Containing expression ends prematurely\" if a closer comes first.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: down_list,
        state: None,
    },
    Command {
        name: "kill-sexp",
        doc: "Kill N balanced expressions after every cursor (before it if N is negative).\n\nExpressions are as in `forward-sexp`. A cursor whose scan fails kills nothing, and the command signals after the others have killed. A kill right after a kill appends to it.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: kill_sexp,
        state: None,
    },
    Command {
        name: "backward-kill-sexp",
        doc: "Kill N balanced expressions before every cursor (after it if N is negative).\n\nSee `kill-sexp`. Appended to a kill before it, the text goes first.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: backward_kill_sexp,
        state: None,
    },
    Command {
        name: "mark-sexp",
        doc: "Set the mark at every cursor N balanced expressions on (back if N is negative), and show the region.\n\nThe cursor stays, and the old mark goes first in the mark ring. Repeated, or while the region shows, move the mark N more expressions on, from where it is: with no argument, away from point. A cursor whose scan fails keeps its mark; if every cursor's fails, nothing changes.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: mark_sexp,
        state: None,
    },
    Command {
        name: "transpose-sexps",
        doc: "Swap the balanced expression before every cursor with the one after it, and leave point after both.\n\nWith N, drag the expression before point past N expressions (back if N is negative). With 0, swap the expression that ends at point with the one that ends at the mark; point and the mark go with their expressions. With no two expressions, signal \"Don't have two things to transpose\". A cursor whose expressions overlap another's swaps nothing.",
        args: ArgSpec::Count,
        reach: Reach::Change,
        run: transpose_sexps,
        state: None,
    },
];

/// The signal for a command that would copy more text than one command inserts.
const TOO_MUCH_TEXT: &str = "Too much text to transpose";

/// How `buffer`'s text reads as expressions, from its `display` snapshot. A buffer that may
/// hold secrets, or with font-lock off, has no highlights, and each line is lexed.
fn syntax_of<'a>(buffer: &'a Buffer, display: &'a DisplaySnapshot) -> Syntax<'a> {
    Syntax::new(display, buffer.highlights(), Table::of(buffer.mode()))
}

/// A command's signal for a scan's error.
fn signal(error: Option<ScanError>) -> Result<(), CommandError> {
    error.map_or(Ok(()), |error| Err(CommandError::new(error.message())))
}

/// An expression motion of `oso_core::sexp`.
type Motion = fn(&Syntax<'_>, &mut Selection, isize, &mut WalkBudget) -> Result<(), ScanError>;

/// Moves every cursor of the selected window `n` times with `motion`, selecting with Shift.
/// Asks for a frame if a cursor moved, and fails with the first cursor's error after the
/// others have moved.
fn move_by(ctx: &mut Ctx<'_>, motion: Motion, n: isize) -> Result<(), CommandError> {
    shift_select(ctx);
    let mut budget = WalkBudget::for_cursors(ctx.editor.window().selections.len());
    let (window, buffer) = ctx.editor.current_mut();
    let display = buffer.display();
    let syntax = syntax_of(buffer, &display);
    let update = window
        .selections
        .update(|selection| motion(&syntax, selection, n, &mut budget));
    if update.changed {
        ctx.request_redraw();
    }
    signal(update.error)
}

fn forward_sexp(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, sexp::forward_sexp, args.count())
}

fn backward_sexp(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, sexp::forward_sexp, args.count().saturating_neg())
}

fn forward_list(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, sexp::forward_list, args.count())
}

fn backward_list(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, sexp::forward_list, args.count().saturating_neg())
}

fn backward_up_list(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, sexp::up_list, args.count().saturating_neg())
}

fn up_list(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, sexp::up_list, args.count())
}

fn down_list(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, sexp::down_list, args.count())
}

/// Kills from every cursor across `n` expressions, backward if `n` is negative.
fn kill_sexps(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    let mut error = None;
    let ranges: Vec<Range<usize>> = {
        let (window, buffer) = ctx.editor.current();
        let display = buffer.display();
        let syntax = syntax_of(buffer, &display);
        let mut budget = WalkBudget::for_cursors(window.selections.len());
        window
            .selections
            .iter()
            .map(
                |selection| match sexp::sexp_end(&syntax, selection.cursor, n, &mut budget) {
                    Ok(to) => selection.cursor.min(to)..selection.cursor.max(to),
                    Err(e) => {
                        error.get_or_insert(e);
                        selection.cursor..selection.cursor
                    }
                },
            )
            .collect()
    };
    kill(ctx, ranges, direction_of(n))?;
    signal(error)
}

fn kill_sexp(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    kill_sexps(ctx, args.count())
}

fn backward_kill_sexp(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    kill_sexps(ctx, args.count().saturating_neg())
}

fn mark_sexp(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let repeated = ctx.repeats() > 0;
    let (window, buffer) = ctx.editor.current_mut();
    let selections = &mut window.selections;
    // As Emacs's `allow-extend`: repeated with a mark, or while the region shows.
    let extend =
        (repeated && selections.primary().mark.is_some()) || selections.mark_active().is_active();
    let display = buffer.display();
    let syntax = syntax_of(buffer, &display);
    let mut budget = WalkBudget::for_cursors(selections.len());
    let old: Vec<Option<usize>> = selections.iter().map(|selection| selection.mark).collect();
    let targets: Vec<Result<usize, ScanError>> = selections
        .iter()
        .map(|selection| {
            let (from, n) = match (extend, selection.mark) {
                (true, Some(mark)) => {
                    // With no argument, the mark goes on away from point.
                    let n = match args.raw() {
                        Some(_) => args.count(),
                        None if mark < selection.cursor => -1,
                        None => 1,
                    };
                    (mark, n)
                }
                _ => (selection.cursor, args.count()),
            };
            sexp::sexp_end(&syntax, from, n, &mut budget)
        })
        .collect();
    let first_error = targets.iter().find_map(|target| target.err());
    if targets.iter().all(Result::is_err) {
        return signal(first_error);
    }
    if !extend {
        selections.push_marks();
    }
    let mut each = targets.iter().zip(old);
    selections.update(|selection| {
        if let Some((target, old)) = each.next() {
            selection.mark = match target {
                Ok(to) => Some(*to),
                Err(_) => old,
            };
        }
        Ok::<(), ()>(())
    });
    selections.set_mark_active(MarkActive::Active);
    ctx.request_redraw();
    if !extend {
        ctx.editor.message("Mark set");
    }
    signal(first_error)
}

/// One cursor's swap: the text from the first expression's start to the second's end, and
/// where its cursor and mark go after it.
struct Swap {
    range: Range<usize>,
    /// The second expression's start and the first's end, in the range.
    middle: Range<usize>,
    cursor: usize,
    mark: Option<usize>,
}

impl Swap {
    /// The swap of `first` and `second`, in the text's order, with the cursor and the mark
    /// where `place` puts them from the two ranges.
    fn of(
        first: Range<usize>,
        second: Range<usize>,
        place: impl FnOnce(&Range<usize>, &Range<usize>) -> (usize, Option<usize>),
    ) -> Self {
        let (cursor, mark) = place(&first, &second);
        Self {
            range: first.start..second.end,
            middle: first.end..second.start,
            cursor,
            mark,
        }
    }
}

/// The expression that ends at `offset`, as `transpose-sexps` with 0 reads it: from the
/// start of the one before it to that start's end, which must not pass `offset`.
fn ending_at(
    syntax: &Syntax<'_>,
    offset: usize,
    budget: &mut WalkBudget,
) -> Result<Range<usize>, ScanError> {
    let start = sexp::sexp_end(syntax, offset, -1, budget)?;
    let end = sexp::sexp_end(syntax, start, 1, budget)?;
    if start >= end || end > offset {
        return Err(ScanError::NothingToTranspose);
    }
    Ok(start..end)
}

/// The swap `transpose-sexps` makes at `selection` with the count `n`.
fn swap_at(
    syntax: &Syntax<'_>,
    selection: &Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<Swap, ScanError> {
    if n != 0 {
        let (first, second) = sexp::transpose_ranges(syntax, selection.cursor, n, budget)?;
        // Point after the one dragged: the second's end going forward, and the end of what
        // took the first's place going back.
        return Ok(Swap::of(first, second, |first, second| {
            let cursor = if n > 0 {
                second.end
            } else {
                first.start + second.len()
            };
            (cursor, None)
        }));
    }
    let mark = selection.mark.ok_or(ScanError::NothingToTranspose)?;
    let at_cursor = ending_at(syntax, selection.cursor, budget)?;
    let at_mark = ending_at(syntax, mark, budget)?;
    let cursor_first = at_cursor.start < at_mark.start;
    let (first, second) = if cursor_first {
        (at_cursor, at_mark)
    } else {
        (at_mark, at_cursor)
    };
    if first.end > second.start {
        return Err(ScanError::NothingToTranspose);
    }
    // Each goes with its expression: the first's to the second's place, and back.
    Ok(Swap::of(first, second, |first, second| {
        let (to_second, to_first) = (second.end, first.start + second.len());
        if cursor_first {
            (to_second, Some(to_first))
        } else {
            (to_first, Some(to_second))
        }
    }))
}

fn transpose_sexps(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let n = args.count();
    let (window, buffer) = ctx.editor.current();
    if n == 0 && window.selections.primary().mark.is_none() {
        return Err(CommandError::new(NO_MARK));
    }
    let display = buffer.display();
    let syntax = syntax_of(buffer, &display);
    let mut budget = WalkBudget::for_cursors(window.selections.len());
    let mut error = None;
    let mut swaps: Vec<Swap> = Vec::new();
    for selection in window.selections.iter() {
        match swap_at(&syntax, selection, n, &mut budget) {
            Ok(swap) => swaps.push(swap),
            // Any scan that finds no expression where one should be says so, as Emacs's
            // interactive `transpose-sexps` does; only the budget says otherwise.
            Err(ScanError::TooFar) => {
                error.get_or_insert(ScanError::TooFar);
            }
            Err(_) => {
                error.get_or_insert(ScanError::NothingToTranspose);
            }
        }
    }
    // A swap that overlaps one before it is left, so that each swaps text of its own.
    swaps.sort_by_key(|swap| swap.range.start);
    let mut end = 0;
    swaps.retain(|swap| {
        let apart = swap.range.start >= end;
        if apart {
            end = swap.range.end;
        }
        apart
    });
    if swaps.is_empty() {
        return signal(error);
    }
    let copied = swaps
        .iter()
        .try_fold(0usize, |sum, swap| sum.checked_add(swap.range.len()));
    if copied.is_none_or(|bytes| bytes > MAX_INSERT) {
        return Err(CommandError::new(TOO_MUCH_TEXT));
    }
    let rope = buffer.text();
    let piece = |range: Range<usize>| -> String { rope.chunks_in_range(range).collect() };
    let texts: Vec<String> = swaps
        .iter()
        .map(|swap| {
            let (range, middle) = (swap.range.clone(), swap.middle.clone());
            let mut text = piece(middle.end..range.end);
            text.push_str(&piece(middle.clone()));
            text.push_str(&piece(range.start..middle.start));
            text
        })
        .collect();
    let edits: Vec<Replacement<'_>> = swaps
        .iter()
        .zip(&texts)
        .map(|(swap, text)| Replacement {
            range: swap.range.clone(),
            text,
        })
        .collect();
    let id = window.buffer();
    ctx.editor.apply_edits(id, &edits)?;
    ctx.request_redraw();
    // A swap keeps the text's length, so offsets after it are what they were. Each cursor
    // that swapped ends after its range's new text; from there it goes where its swap says.
    let selections = &mut ctx.editor.current_mut().0.selections;
    selections.update(|selection| {
        let found = swaps.binary_search_by_key(&selection.cursor, |swap| swap.range.end);
        if let Ok(index) = found {
            let swap = &swaps[index];
            selection.move_to(swap.cursor);
            if swap.mark.is_some() {
                selection.mark = swap.mark;
            }
        }
        Ok::<(), ()>(())
    });
    signal(error)
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::{Buffer, Mode};
    use oso_core::command::{Args, Effects, Reach};
    use oso_core::editor::Editor;
    use oso_core::selection::{MarkActive, Selection, SelectionSet};
    use oso_core::window::{Scroll, ViewSize};

    use crate::commands::registry;
    use crate::commands::tests::{cursor, editor, run};

    /// An editor on `text` in Rust mode, with no highlights yet.
    fn rust(text: &str) -> Editor {
        let mut buffer = Buffer::from_text("t.rs", text);
        assert!(buffer.set_mode(Mode::Rust));
        let mut editor = Editor::with_buffer(buffer);
        editor.set_view_size(ViewSize::new(10, 40));
        editor
    }

    /// Puts a cursor at each of `cursors`, the first the primary, with no mark.
    fn place(editor: &mut Editor, cursors: &[usize]) {
        let selections = cursors.iter().map(|at| Selection::at(*at)).collect();
        let set = SelectionSet::from_parts(selections, 0).unwrap();
        editor.restore_positions(set, Scroll::default());
    }

    fn cursors(editor: &Editor) -> Vec<usize> {
        editor
            .window()
            .selections
            .iter()
            .map(|s| s.cursor)
            .collect()
    }

    fn mark(editor: &Editor) -> Option<usize> {
        editor.window().selections.primary().mark
    }

    fn text(editor: &Editor) -> String {
        let rope = editor.current().1.text();
        rope.chunks_in_range(0..rope.len()).collect()
    }

    fn newest_kill(editor: &Editor) -> String {
        editor
            .kill_ring()
            .get(0)
            .map(|entry| entry.joined())
            .unwrap_or_default()
    }

    #[test]
    fn forward_and_backward_sexp_take_a_count_either_way() {
        let mut editor = editor("(a b) foo [c]");
        assert!(run(&mut editor, "forward-sexp", Args::NONE).1.redraw);
        assert_eq!(cursor(&editor), 5);
        let _ = run(&mut editor, "forward-sexp", Args::numeric(2));
        assert_eq!(cursor(&editor), 13);
        assert_eq!(
            run(&mut editor, "forward-sexp", Args::NONE),
            (Ok(()), Effects::default()),
            "past the last, the buffer's end, and no signal"
        );
        let _ = run(&mut editor, "backward-sexp", Args::NONE);
        assert_eq!(cursor(&editor), 10);
        let _ = run(&mut editor, "forward-sexp", Args::numeric(-2));
        assert_eq!(cursor(&editor), 0);
        let _ = run(&mut editor, "backward-sexp", Args::numeric(-1));
        assert_eq!(cursor(&editor), 5);
        assert_eq!(
            run(&mut editor, "forward-sexp", Args::numeric(0)),
            (Ok(()), Effects::default())
        );
        let (result, _) = run(&mut editor, "backward-sexp", Args::numeric(isize::MAX));
        assert_eq!((result, cursor(&editor)), (Ok(()), 0));
        let (result, _) = run(&mut editor, "forward-sexp", Args::numeric(isize::MIN));
        assert_eq!((result, cursor(&editor)), (Ok(()), 0));
    }

    #[test]
    fn the_errors_are_emacss_and_the_cursor_stays() {
        let mut editor = editor("(a)");
        place(&mut editor, &[2]);
        assert_eq!(
            run(&mut editor, "forward-sexp", Args::NONE),
            (
                Err("Containing expression ends prematurely".into()),
                Effects::default()
            )
        );
        assert_eq!(cursor(&editor), 2);
        let mut editor = crate::commands::tests::editor("(a");
        let (result, _) = run(&mut editor, "forward-sexp", Args::NONE);
        assert_eq!(result, Err("Unbalanced parentheses".into()));
        assert_eq!(cursor(&editor), 0);
    }

    #[test]
    fn strings_come_from_the_modes_table() {
        // A paren in a Rust string is no paren; in Fundamental there are no strings.
        let source = "f(\"a)\") x";
        let mut code = rust(source);
        place(&mut code, &[1]);
        let _ = run(&mut code, "forward-sexp", Args::NONE);
        assert_eq!(cursor(&code), 7);
        let mut plain = editor(source);
        place(&mut plain, &[1]);
        let _ = run(&mut plain, "forward-sexp", Args::NONE);
        assert_eq!(cursor(&plain), 5);
    }

    #[test]
    fn forward_and_backward_list_pass_what_is_not_a_list() {
        let mut editor = editor("a (b) c [d] e");
        let _ = run(&mut editor, "forward-list", Args::NONE);
        assert_eq!(cursor(&editor), 5);
        let _ = run(&mut editor, "forward-list", Args::NONE);
        assert_eq!(cursor(&editor), 11);
        let _ = run(&mut editor, "backward-list", Args::NONE);
        assert_eq!(cursor(&editor), 8);
        let _ = run(&mut editor, "backward-list", Args::numeric(-1));
        assert_eq!(cursor(&editor), 11);
        let _ = run(&mut editor, "forward-list", Args::NONE);
        assert_eq!(cursor(&editor), 13, "past the last, the buffer's end");
    }

    #[test]
    fn up_and_out_of_lists() {
        let mut editor = editor("(a [bc] d)");
        place(&mut editor, &[5]);
        let _ = run(&mut editor, "backward-up-list", Args::NONE);
        assert_eq!(cursor(&editor), 3);
        let _ = run(&mut editor, "backward-up-list", Args::NONE);
        assert_eq!(cursor(&editor), 0);
        assert_eq!(
            run(&mut editor, "backward-up-list", Args::NONE),
            (Err("Unbalanced parentheses".into()), Effects::default())
        );
        place(&mut editor, &[5]);
        let _ = run(&mut editor, "up-list", Args::NONE);
        assert_eq!(cursor(&editor), 7);
        place(&mut editor, &[5]);
        let _ = run(&mut editor, "up-list", Args::numeric(2));
        assert_eq!(cursor(&editor), 10);
        place(&mut editor, &[5]);
        let _ = run(&mut editor, "backward-up-list", Args::numeric(-1));
        assert_eq!(cursor(&editor), 7, "a negative count goes forward");
    }

    #[test]
    fn up_list_leaves_a_string_first() {
        let mut editor = rust("(\"ab\")");
        place(&mut editor, &[3]);
        let _ = run(&mut editor, "backward-up-list", Args::NONE);
        assert_eq!(cursor(&editor), 1);
        place(&mut editor, &[3]);
        let _ = run(&mut editor, "up-list", Args::NONE);
        assert_eq!(cursor(&editor), 5);
    }

    #[test]
    fn down_list_goes_in_and_backward_stops_before_the_closer() {
        let mut editor = editor("a (b [c])");
        let _ = run(&mut editor, "down-list", Args::NONE);
        assert_eq!(cursor(&editor), 3);
        let _ = run(&mut editor, "down-list", Args::NONE);
        assert_eq!(cursor(&editor), 6);
        assert_eq!(
            run(&mut editor, "down-list", Args::NONE).0,
            Err("Containing expression ends prematurely".into())
        );
        assert_eq!(cursor(&editor), 6);
        place(&mut editor, &[9]);
        let _ = run(&mut editor, "down-list", Args::numeric(-1));
        assert_eq!(cursor(&editor), 8);
        place(&mut editor, &[9]);
        let _ = run(&mut editor, "down-list", Args::numeric(-2));
        assert_eq!(cursor(&editor), 7);
    }

    #[test]
    fn every_cursor_moves_on_its_own_and_the_first_error_shows_after() {
        let mut editor = editor("(a) (b c)");
        place(&mut editor, &[0, 6]);
        let _ = run(&mut editor, "forward-sexp", Args::NONE);
        assert_eq!(cursors(&editor), [3, 8]);
        let (result, effects) = run(&mut editor, "forward-sexp", Args::NONE);
        assert_eq!(result, Err("Containing expression ends prematurely".into()));
        assert!(effects.redraw, "the first cursor moved");
        assert_eq!(cursors(&editor), [8, 9]);
        // Cursors that meet merge.
        place(&mut editor, &[5, 7]);
        let _ = run(&mut editor, "backward-up-list", Args::NONE);
        assert_eq!(cursors(&editor), [4]);
    }

    #[test]
    fn a_motion_keeps_a_shown_region_and_grows_it() {
        let mut editor = editor("(a) (b) c");
        let _ = run(&mut editor, "set-mark-command", Args::NONE);
        let _ = run(&mut editor, "forward-sexp", Args::numeric(2));
        let selections = &editor.window().selections;
        assert_eq!(selections.mark_active(), MarkActive::Active);
        assert_eq!(selections.primary().range(), 0..7);
    }

    #[test]
    fn a_scan_past_its_budget_leaves_the_cursor_and_says_so() {
        let mut editor = editor(&"(".repeat(8 << 20));
        let (result, effects) = run(&mut editor, "forward-sexp", Args::NONE);
        assert_eq!(result, Err("Count too large".into()));
        assert_eq!(effects, Effects::default());
        assert_eq!(cursor(&editor), 0);
    }

    #[test]
    fn kill_sexp_kills_n_expressions_either_way() {
        let mut editor = editor("(a b) c d");
        let _ = run(&mut editor, "kill-sexp", Args::NONE);
        assert_eq!(text(&editor), " c d");
        assert_eq!(newest_kill(&editor), "(a b)");
        let mut editor = crate::commands::tests::editor("(a b) c d");
        let _ = run(&mut editor, "kill-sexp", Args::numeric(2));
        assert_eq!(text(&editor), " d");
        let mut editor = crate::commands::tests::editor("x (a) y");
        place(&mut editor, &[5]);
        let (result, effects) = run(&mut editor, "backward-kill-sexp", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        assert_eq!((text(&editor), cursor(&editor)), ("x  y".into(), 2));
        assert_eq!(newest_kill(&editor), "(a)");
        let mut editor = crate::commands::tests::editor("a b c");
        place(&mut editor, &[5]);
        let _ = run(&mut editor, "kill-sexp", Args::numeric(-2));
        assert_eq!(text(&editor), "a ");
    }

    #[test]
    fn a_kill_that_cannot_scan_kills_nothing_at_that_cursor() {
        let mut editor = editor("(a) (b");
        place(&mut editor, &[0, 4]);
        let (result, effects) = run(&mut editor, "kill-sexp", Args::NONE);
        assert_eq!(result, Err("Unbalanced parentheses".into()));
        assert!(effects.redraw, "the first cursor killed");
        assert_eq!(text(&editor), " (b");
        let mut editor = crate::commands::tests::editor("(a");
        assert_eq!(
            run(&mut editor, "kill-sexp", Args::NONE),
            (Err("Unbalanced parentheses".into()), Effects::default())
        );
        assert_eq!(text(&editor), "(a");
    }

    #[test]
    fn kill_sexp_kills_at_every_cursor() {
        let mut editor = editor("(a) x\n(b) y");
        place(&mut editor, &[0, 6]);
        let _ = run(&mut editor, "kill-sexp", Args::NONE);
        assert_eq!(text(&editor), " x\n y");
        assert_eq!(cursors(&editor), [0, 3]);
    }

    #[test]
    fn mark_sexp_marks_and_then_moves_the_mark_on() {
        let mut editor = editor("x (a) b c");
        place(&mut editor, &[1]);
        editor.current_mut().0.selections.push_primary_mark(0);
        let (result, effects) = run(&mut editor, "mark-sexp", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        assert_eq!((cursor(&editor), mark(&editor)), (1, Some(5)));
        let selections = &editor.window().selections;
        assert_eq!(selections.mark_active(), MarkActive::Active);
        assert_eq!(
            selections.mark_ring().iter().next(),
            Some(0),
            "the old mark went to the ring"
        );
        assert_eq!(
            crate::commands::kept_messages(&editor)
                .last()
                .map(String::as_str),
            Some("Mark set")
        );
        // While the region shows, the mark goes on.
        let _ = run(&mut editor, "mark-sexp", Args::NONE);
        assert_eq!((cursor(&editor), mark(&editor)), (1, Some(7)));
        let _ = run(&mut editor, "mark-sexp", Args::numeric(-1));
        assert_eq!(mark(&editor), Some(6));
    }

    #[test]
    fn mark_sexp_backward_goes_on_backward() {
        let mut editor = editor("a b c");
        place(&mut editor, &[5]);
        let _ = run(&mut editor, "mark-sexp", Args::numeric(-1));
        assert_eq!(mark(&editor), Some(4));
        let _ = run(&mut editor, "mark-sexp", Args::NONE);
        assert_eq!(mark(&editor), Some(2), "the mark before point goes on back");
    }

    #[test]
    fn mark_sexp_marks_at_every_cursor_and_fails_with_none_changed() {
        let mut editor = editor("(a) (b) c");
        place(&mut editor, &[0, 4]);
        let _ = run(&mut editor, "mark-sexp", Args::NONE);
        let marks: Vec<Option<usize>> = editor.window().selections.iter().map(|s| s.mark).collect();
        assert_eq!(marks, [Some(3), Some(7)]);
        let mut editor = crate::commands::tests::editor("(a");
        assert_eq!(
            run(&mut editor, "mark-sexp", Args::NONE),
            (Err("Unbalanced parentheses".into()), Effects::default())
        );
        assert_eq!(mark(&editor), None);
    }

    #[test]
    fn transpose_sexps_drags_the_one_before_point() {
        let mut editor = editor("a (b c)");
        place(&mut editor, &[1]);
        let (result, effects) = run(&mut editor, "transpose-sexps", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        assert_eq!((text(&editor), cursor(&editor)), ("(b c) a".into(), 7));
        let mut editor = crate::commands::tests::editor("a b c");
        place(&mut editor, &[1]);
        let _ = run(&mut editor, "transpose-sexps", Args::numeric(2));
        assert_eq!((text(&editor), cursor(&editor)), ("b c a".into(), 5));
        let mut editor = crate::commands::tests::editor("a bb c");
        place(&mut editor, &[6]);
        let _ = run(&mut editor, "transpose-sexps", Args::numeric(-1));
        assert_eq!((text(&editor), cursor(&editor)), ("a c bb".into(), 3));
    }

    #[test]
    fn transpose_sexps_with_zero_swaps_at_point_and_mark() {
        let mut editor = editor("foo bar bazz");
        place(&mut editor, &[12]);
        editor.current_mut().0.selections.push_primary_mark(7);
        let _ = run(&mut editor, "transpose-sexps", Args::numeric(0));
        assert_eq!(text(&editor), "foo bazz bar");
        assert_eq!((cursor(&editor), mark(&editor)), (8, Some(12)));
        let mut editor = crate::commands::tests::editor("a b");
        assert_eq!(
            run(&mut editor, "transpose-sexps", Args::numeric(0)),
            (Err("No mark set in this buffer".into()), Effects::default())
        );
    }

    #[test]
    fn transpose_sexps_at_every_cursor_and_with_nothing_to_swap() {
        let mut editor = editor("a b\nc dd");
        place(&mut editor, &[1, 5]);
        let _ = run(&mut editor, "transpose-sexps", Args::NONE);
        assert_eq!(text(&editor), "b a\ndd c");
        assert_eq!(cursors(&editor), [3, 8]);
        let mut editor = crate::commands::tests::editor("a");
        place(&mut editor, &[1]);
        assert_eq!(
            run(&mut editor, "transpose-sexps", Args::NONE),
            (
                Err("Don't have two things to transpose".into()),
                Effects::default()
            )
        );
        assert_eq!(text(&editor), "a");
        // Two cursors that would swap the same expression: the first swaps alone.
        let mut editor = crate::commands::tests::editor("a b c");
        place(&mut editor, &[1, 3]);
        assert_eq!(run(&mut editor, "transpose-sexps", Args::NONE).0, Ok(()));
        assert_eq!(text(&editor), "b a c");
        // A closer where the second would be is no second either.
        let mut editor = crate::commands::tests::editor("(a)");
        place(&mut editor, &[2]);
        assert_eq!(
            run(&mut editor, "transpose-sexps", Args::NONE).0,
            Err("Don't have two things to transpose".into())
        );
    }

    #[test]
    fn the_agent_channel_may_look_with_the_motions_and_change_with_the_edits() {
        let registry = registry();
        let reach = |name: &str| {
            let id = registry.lookup(name).unwrap();
            registry.iter().find(|(at, _)| *at == id).unwrap().1.reach
        };
        for name in [
            "forward-sexp",
            "backward-sexp",
            "forward-list",
            "backward-list",
            "backward-up-list",
            "up-list",
            "down-list",
            "mark-sexp",
        ] {
            assert_eq!(reach(name), Reach::Look, "{name}");
        }
        for name in ["kill-sexp", "backward-kill-sexp", "transpose-sexps"] {
            assert_eq!(reach(name), Reach::Change, "{name}");
        }
    }
}
