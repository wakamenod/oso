//! Commands that move the cursors: by character, word and line, and to the ends of
//! lines and the buffer (ADR 0030, ADR 0040, ADR 0058).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! Motion commands move every cursor of the selected window (ADR 0007).
//! After each command the dispatcher scrolls the window to keep its cursor
//! in view (`Editor::follow_cursor`).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::display_map::DisplaySnapshot;
use oso_core::motion::{self, Edge, WalkBudget};
use oso_core::selection::Selection;
use oso_core::word;

use super::budget_left;

/// The motion commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "forward-char",
        doc: "Move point N characters forward (backward if N is negative).\n\nA character is what the screen shows as one: a grapheme cluster such as `が` or an emoji sequence, a tab, a line break or an escape. At the end of the buffer, signal \"End of buffer\". A count that would read too much goes as far as it may and signals \"Count too large\".",
        args: ArgSpec::Count,
        run: forward_char,
    },
    Command {
        name: "backward-char",
        doc: "Move point N characters backward (forward if N is negative).\n\nCharacters are counted as in `forward-char`. At the start of the buffer, signal \"Beginning of buffer\".",
        args: ArgSpec::Count,
        run: backward_char,
    },
    Command {
        name: "forward-word",
        doc: "Move point to the end of the next word, N times (backward if N is negative).\n\nPoint passes what is not part of a word first. Words break where the script changes, but kanji followed by kana stays one word, and at camelCase boundaries. A very long run with no boundary stops partway, and the next key goes on. At the end of the buffer, stay there. A count that would read too much goes as far as it may and signals \"Count too large\".",
        args: ArgSpec::Count,
        run: forward_word,
    },
    Command {
        name: "backward-word",
        doc: "Move point to the start of the previous word, N times (forward if N is negative).\n\nSee `forward-word`.",
        args: ArgSpec::Count,
        run: backward_word,
    },
    Command {
        name: "next-line",
        doc: "Move point N lines down (up if N is negative), keeping the goal column.\n\nThe goal column, in display cells, is set by the first of consecutive line motions. Started at the end of a line, the motion stays at line ends. On the last line, move to the end of the buffer and signal \"End of buffer\".",
        args: ArgSpec::Count,
        run: next_line,
    },
    Command {
        name: "previous-line",
        doc: "Move point N lines up (down if N is negative), keeping the goal column.\n\nSee `next-line`. On the first line, move to the start of the buffer and signal \"Beginning of buffer\".",
        args: ArgSpec::Count,
        run: previous_line,
    },
    Command {
        name: "move-beginning-of-line",
        doc: "Move point to the beginning of the line.\n\nWith N other than 1, move N - 1 lines down first.",
        args: ArgSpec::Count,
        run: move_beginning_of_line,
    },
    Command {
        name: "move-end-of-line",
        doc: "Move point to the end of the line, before its line break.\n\nWith N other than 1, move N - 1 lines down first.",
        args: ArgSpec::Count,
        run: move_end_of_line,
    },
    Command {
        name: "mwim-beginning-of-code-or-line",
        doc: "Move point to the first non-blank character of the line, or from there to the line's start.\n\nWith N other than 1, move N - 1 lines down first, and to the first non-blank character.",
        args: ArgSpec::Count,
        run: mwim_beginning,
    },
    Command {
        name: "mwim-end-of-code-or-line",
        doc: "Move point to the end of the line before trailing blanks, or from there to the line's end.\n\nWith N other than 1, move N - 1 lines down first, and to the end before the blanks.",
        args: ArgSpec::Count,
        run: mwim_end,
    },
    Command {
        name: "beginning-of-buffer",
        doc: "Move point to the beginning of the buffer.\n\nWith a numeric argument N, move to the line after N/10 of the way from the beginning.",
        args: ArgSpec::Count,
        run: beginning_of_buffer,
    },
    Command {
        name: "end-of-buffer",
        doc: "Move point to the end of the buffer.\n\nWith a numeric argument N, move to the line after N/10 of the way from the end.",
        args: ArgSpec::Count,
        run: end_of_buffer,
    },
];

/// Moves every cursor of the selected window with `motion`. Asks for a
/// redraw if a cursor moved, and fails with the first buffer edge a cursor
/// hit, after moving them all as far as they go.
fn move_cursors(
    ctx: &mut Ctx<'_>,
    mut motion: impl FnMut(&DisplaySnapshot, &mut Selection) -> Result<(), Edge>,
) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current_mut();
    let display = buffer.display();
    let update = window
        .selections
        .update(|selection| motion(&display, selection));
    if update.changed {
        ctx.request_redraw();
    }
    match update.error {
        Some(edge) => Err(CommandError::new(edge.message())),
        None => Ok(()),
    }
}

/// Moves every cursor by `n` clusters. A count that reads too much leaves them where
/// it got to and signals.
fn move_chars(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    let mut budget = WalkBudget::new();
    move_cursors(ctx, |display, s| {
        motion::move_clusters(display, s, n, &mut budget)
    })?;
    budget_left(budget)
}

fn forward_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_chars(ctx, args.count())
}

fn backward_char(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_chars(ctx, args.count().saturating_neg())
}

/// Moves every cursor by `n` words. A count that reads too much leaves them where
/// it got to and signals. The buffer's edge does not, as in Emacs.
fn move_words(ctx: &mut Ctx<'_>, n: isize) -> Result<(), CommandError> {
    let mut budget = WalkBudget::new();
    move_cursors(ctx, |display, s| {
        *s = Selection::at(word::move_words(display, s.cursor, n, &mut budget));
        Ok(())
    })?;
    budget_left(budget)
}

fn forward_word(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_words(ctx, args.count())
}

fn backward_word(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_words(ctx, args.count().saturating_neg())
}

fn next_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_cursors(ctx, |display, s| {
        motion::move_lines(display, s, args.count())
    })
}

fn previous_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let n = args.count().saturating_neg();
    move_cursors(ctx, |display, s| motion::move_lines(display, s, n))
}

fn move_beginning_of_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_cursors(ctx, |display, s| {
        motion::move_line_start(display, s, args.count());
        Ok(())
    })
}

fn move_end_of_line(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_cursors(ctx, |display, s| {
        motion::move_line_end(display, s, args.count());
        Ok(())
    })
}

fn mwim_beginning(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let mut budget = WalkBudget::new();
    move_cursors(ctx, |display, s| {
        motion::toggle_line_start(display, s, args.count(), &mut budget);
        Ok(())
    })?;
    budget_left(budget)
}

fn mwim_end(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let mut budget = WalkBudget::new();
    move_cursors(ctx, |display, s| {
        motion::toggle_line_end(display, s, args.count(), &mut budget);
        Ok(())
    })?;
    budget_left(budget)
}

/// `n` tenths, clamped to 0 to 10.
fn tenths(n: isize) -> usize {
    n.clamp(0, 10) as usize
}

fn beginning_of_buffer(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    // Pushing the mark comes with marks (M2).
    move_cursors(ctx, |display, s| {
        let len = motion::buffer_end(display);
        *s = Selection::at(match args.prefix() {
            None => 0,
            Some(n) => motion::line_after_fraction(display, len, tenths(n)),
        });
        Ok(())
    })
}

fn end_of_buffer(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_cursors(ctx, |display, s| {
        let len = motion::buffer_end(display);
        *s = Selection::at(match args.prefix() {
            None => len,
            Some(n) => motion::line_after_fraction(display, len, 10 - tenths(n)),
        });
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{cursor, editor, run};
    use oso_core::command::{Args, Effects};

    #[test]
    fn char_motion_moves_by_cluster_and_redraws() {
        let mut editor = editor("aか\u{3099}🇯🇵b");
        assert!(run(&mut editor, "forward-char", Args::NONE).1.redraw);
        assert_eq!(cursor(&editor), 1);
        let _ = run(&mut editor, "forward-char", Args::numeric(2));
        assert_eq!(cursor(&editor), 1 + 6 + 8);
        let _ = run(&mut editor, "backward-char", Args::numeric(-1));
        assert_eq!(cursor(&editor), 16);
        let _ = run(&mut editor, "backward-char", Args::numeric(3));
        assert_eq!(cursor(&editor), 1);
    }

    #[test]
    fn char_motion_at_the_ends_signals_and_does_not_redraw() {
        let mut editor = editor("ab");
        assert_eq!(
            run(&mut editor, "backward-char", Args::NONE),
            (Err("Beginning of buffer".into()), Effects::default())
        );
        assert_eq!(
            run(&mut editor, "forward-char", Args::numeric(0)),
            (Ok(()), Effects::default())
        );
        let (result, effects) = run(&mut editor, "forward-char", Args::numeric(isize::MAX));
        assert_eq!(result, Err("End of buffer".into()));
        assert!(effects.redraw, "it moved as far as it could");
        assert_eq!(cursor(&editor), 2);
        assert_eq!(
            run(&mut editor, "backward-char", Args::numeric(isize::MIN)),
            (Err("End of buffer".into()), Effects::default())
        );
    }

    #[test]
    fn a_count_that_reads_too_much_stops_and_signals_and_past_the_edge_needs_no_walk() {
        // Line breaks cost the most to read.
        let len = 1 << 15;
        let mut editor = editor(&"\n".repeat(len));
        let (result, effects) = run(&mut editor, "forward-char", Args::numeric(1 << 15));
        assert_eq!(result, Err("Count too large".into()));
        assert!(effects.redraw);
        let stopped = cursor(&editor);
        assert!(stopped > 0 && stopped < len, "{stopped}");
        let (result, _) = run(&mut editor, "forward-char", Args::numeric(isize::MAX));
        assert_eq!(result, Err("End of buffer".into()));
        assert_eq!(cursor(&editor), len, "it went to the end");
        let (result, _) = run(&mut editor, "backward-char", Args::numeric(1 << 15));
        assert_eq!(result, Err("Count too large".into()));
        assert!(cursor(&editor) > 0);
    }

    #[test]
    fn a_large_count_on_a_long_line_goes_all_the_way() {
        let mut editor = editor(&"x".repeat(100_000));
        let (result, _) = run(&mut editor, "forward-char", Args::numeric(99_000));
        assert_eq!((result, cursor(&editor)), (Ok(()), 99_000));
        let (result, _) = run(&mut editor, "backward-char", Args::numeric(90_000));
        assert_eq!((result, cursor(&editor)), (Ok(()), 9_000));
    }

    #[test]
    fn word_motion_counts_and_stays_silent_at_the_edges() {
        let mut editor = editor("one two three");
        assert!(run(&mut editor, "forward-word", Args::NONE).1.redraw);
        assert_eq!(cursor(&editor), 3);
        let _ = run(&mut editor, "forward-word", Args::numeric(2));
        assert_eq!(cursor(&editor), 13);
        assert_eq!(
            run(&mut editor, "forward-word", Args::NONE),
            (Ok(()), Effects::default()),
            "no signal and no frame"
        );
        let _ = run(&mut editor, "forward-word", Args::numeric(-1));
        assert_eq!(cursor(&editor), 8);
        let _ = run(&mut editor, "backward-word", Args::numeric(-1));
        assert_eq!(cursor(&editor), 13);
        assert_eq!(
            run(&mut editor, "backward-word", Args::numeric(0)),
            (Ok(()), Effects::default())
        );
        assert_eq!(
            run(&mut editor, "backward-word", Args::numeric(isize::MAX)).0,
            Ok(())
        );
        assert_eq!(cursor(&editor), 0);
        let (result, _) = run(&mut editor, "forward-word", Args::numeric(isize::MIN));
        assert_eq!((result, cursor(&editor)), (Ok(()), 0));
    }

    #[test]
    fn word_motion_past_what_a_command_reads_stops_where_it_got_to_and_signals() {
        let mut editor = editor(&"a\n".repeat(1 << 15));
        let (result, effects) = run(&mut editor, "forward-word", Args::numeric(isize::MAX));
        assert_eq!(result, Err("Count too large".into()));
        assert!(effects.redraw, "it moved");
        assert!(cursor(&editor) > 0 && cursor(&editor) < 2 << 15);
    }

    #[test]
    fn line_start_toggles_with_the_first_non_blank_character() {
        let mut editor = editor("  \t\u{3000}code here  \nnext");
        let code = "  \t\u{3000}".len();
        // From the start of the line, then from each of its two places, then from
        // elsewhere.
        let name = "mwim-beginning-of-code-or-line";
        assert!(run(&mut editor, name, Args::NONE).1.redraw);
        assert_eq!(cursor(&editor), code);
        let _ = run(&mut editor, name, Args::NONE);
        assert_eq!(cursor(&editor), 0);
        let _ = run(&mut editor, "forward-char", Args::numeric(6));
        let _ = run(&mut editor, name, Args::NONE);
        assert_eq!(cursor(&editor), code);
    }

    #[test]
    fn line_end_toggles_with_the_end_before_trailing_blanks() {
        let text = "code here \t\u{3000}\nnext";
        let mut editor = editor(text);
        let code = "code here".len();
        let end = "code here \t\u{3000}".len();
        let name = "mwim-end-of-code-or-line";
        assert!(run(&mut editor, name, Args::NONE).1.redraw);
        assert_eq!(cursor(&editor), code);
        let _ = run(&mut editor, name, Args::NONE);
        assert_eq!(cursor(&editor), end);
        let _ = run(&mut editor, name, Args::NONE);
        assert_eq!(cursor(&editor), code);
    }

    #[test]
    fn toggling_a_line_with_nothing_to_toggle() {
        let (start, end) = ("mwim-beginning-of-code-or-line", "mwim-end-of-code-or-line");
        // No blanks: one place at each end.
        let mut plain = editor("abc");
        let _ = run(&mut plain, end, Args::NONE);
        assert_eq!(cursor(&plain), 3);
        assert_eq!(
            run(&mut plain, end, Args::NONE),
            (Ok(()), Effects::default())
        );
        let _ = run(&mut plain, start, Args::NONE);
        assert_eq!(cursor(&plain), 0);
        assert_eq!(
            run(&mut plain, start, Args::NONE),
            (Ok(()), Effects::default())
        );
        // Nothing but blanks: the two ends.
        let mut blanks = editor("   ");
        let _ = run(&mut blanks, start, Args::NONE);
        assert_eq!(cursor(&blanks), 3);
        let _ = run(&mut blanks, end, Args::NONE);
        assert_eq!(cursor(&blanks), 0);
        let _ = run(&mut blanks, end, Args::NONE);
        assert_eq!(cursor(&blanks), 3);
        // An empty line.
        let mut empty = editor("");
        assert_eq!(
            run(&mut empty, start, Args::NONE),
            (Ok(()), Effects::default())
        );
        assert_eq!(
            run(&mut empty, end, Args::NONE),
            (Ok(()), Effects::default())
        );
    }

    #[test]
    fn toggling_takes_a_count_of_lines() {
        let (start, end) = ("mwim-beginning-of-code-or-line", "mwim-end-of-code-or-line");
        let mut editor = editor("  a  \n  b  \n  c  ");
        let _ = run(&mut editor, start, Args::numeric(2));
        assert_eq!(cursor(&editor), 6 + 2, "the next line's first non-blank");
        let _ = run(&mut editor, start, Args::numeric(2));
        assert_eq!(cursor(&editor), 12 + 2, "a count does not toggle");
        let _ = run(&mut editor, end, Args::numeric(0));
        assert_eq!(cursor(&editor), 6 + 3, "the line before");
        let _ = run(&mut editor, end, Args::numeric(isize::MAX));
        assert_eq!(cursor(&editor), 12 + 3);
        let _ = run(&mut editor, start, Args::numeric(isize::MIN));
        assert_eq!(cursor(&editor), 2);
    }

    #[test]
    fn line_motion_keeps_the_goal_and_signals_at_the_ends() {
        let mut editor = editor("abcd\nx\nabcd");
        let _ = run(&mut editor, "forward-char", Args::numeric(3));
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 6, "end of the short line");
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 10, "back at column 3");
        let (result, _) = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(result, Err("End of buffer".into()));
        assert_eq!(cursor(&editor), 11);
        let _ = run(&mut editor, "previous-line", Args::numeric(2));
        assert_eq!(cursor(&editor), 3, "the goal column is kept");
        let (result, _) = run(&mut editor, "previous-line", Args::NONE);
        assert_eq!(result, Err("Beginning of buffer".into()));
        assert_eq!(cursor(&editor), 0);
    }

    #[test]
    fn line_motion_stays_at_line_ends_once_there_but_not_from_an_empty_line() {
        let mut editor = editor("abc\n\nabcdef\nabcd");
        let _ = run(&mut editor, "move-end-of-line", Args::NONE);
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 4, "the empty line");
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 11, "the end of the long line");
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 16, "the end of the last");

        // Start at an empty line by motion: column 0 stays the goal.
        let mut editor = crate::commands::tests::editor("abc\n\nabcdef\n");
        let _ = run(&mut editor, "next-line", Args::NONE);
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 5, "from the start of the line above");
        let _ = run(&mut editor, "previous-line", Args::NONE);
        let _ = run(&mut editor, "beginning-of-buffer", Args::NONE);
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 4);
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 5, "goal 0, not the line's end");

        // Going to the end of an empty line on purpose counts, as in Emacs.
        let mut editor = crate::commands::tests::editor("\nabcdef\n");
        let _ = run(&mut editor, "move-end-of-line", Args::NONE);
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 7);
        let mut editor = crate::commands::tests::editor("\nabcdef\n");
        let _ = run(&mut editor, "mwim-end-of-code-or-line", Args::NONE);
        let _ = run(&mut editor, "next-line", Args::NONE);
        assert_eq!(cursor(&editor), 7);
    }

    #[test]
    fn line_and_buffer_ends() {
        let mut editor = editor("ab\r\ncd\nef\ngh\nij\nkl\nmn\nop\nqr\nst\n");
        let _ = run(&mut editor, "move-end-of-line", Args::NONE);
        assert_eq!(cursor(&editor), 2);
        let _ = run(&mut editor, "move-beginning-of-line", Args::numeric(2));
        assert_eq!(cursor(&editor), 4);
        let _ = run(&mut editor, "end-of-buffer", Args::NONE);
        assert_eq!(cursor(&editor), 31);
        let _ = run(&mut editor, "beginning-of-buffer", Args::NONE);
        assert_eq!(cursor(&editor), 0);
        let _ = run(&mut editor, "beginning-of-buffer", Args::numeric(5));
        assert_eq!(cursor(&editor), 16, "the line after the middle");
        let _ = run(&mut editor, "end-of-buffer", Args::numeric(10));
        assert_eq!(cursor(&editor), 4, "the line after the start");
    }
}
