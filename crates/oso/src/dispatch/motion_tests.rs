//! Scenario tests of word motion, mwim, `C-l` and the wheel, with their keys,
//! through the [`Dispatcher`] (ADR 0037, ADR 0058).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every
//! OS with no window.

use oso_core::buffer::Buffer;
use oso_core::command::Effects;
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::selection::{Selection, SelectionSet};
use oso_core::window::{Scroll, ViewSize};

use super::Dispatcher;
use crate::commands::kept_messages;

const REDRAW: Effects = Effects {
    redraw: true,
    exit: false,
    prefix: None,
};
pub(super) const NOTHING: Effects = Effects {
    redraw: false,
    exit: false,
    prefix: None,
};

pub(super) fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) -> Effects {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0)
}

/// Runs isearch's worker jobs in place, as the host's thread would, and takes in their
/// reports. A key searches on its own for a few milliseconds, counted from before it builds
/// the pattern, as a migemo one for a lowercase string: past them, the rest goes to the
/// worker, so a test reads what a search found only after this.
pub(super) fn drain(editor: &mut Editor) {
    while let Some(job) = editor.take_isearch_job() {
        let mut reports = Vec::new();
        job.run(|report| reports.push(report));
        for report in reports {
            editor.isearch_reported(report);
        }
    }
}

/// A dispatcher and an editor showing `text`, with a cursor at each of `cursors`.
pub(super) fn at(text: &str, cursors: &[usize]) -> (Dispatcher, Editor) {
    let mut editor = Editor::with_buffer(Buffer::from_bytes("test", text.as_bytes()));
    editor.set_view_size(ViewSize::new(10, 20));
    place(&mut editor, cursors);
    (Dispatcher::new(), editor)
}

/// Puts a cursor at each of `cursors`, with no mark.
pub(super) fn place_at(editor: &mut Editor, cursors: &[usize]) {
    place(editor, cursors);
}

fn place(editor: &mut Editor, cursors: &[usize]) {
    let selections = cursors.iter().map(|at| Selection::at(*at)).collect();
    let set = SelectionSet::from_parts(selections, 0).unwrap();
    editor.restore_positions(set, Scroll::default());
}

pub(super) fn text(editor: &Editor) -> String {
    let rope = editor.current().1.text();
    rope.chunks_in_range(0..rope.len()).collect()
}

pub(super) fn cursors(editor: &Editor) -> Vec<usize> {
    editor
        .window()
        .selections
        .iter()
        .map(|s| s.cursor)
        .collect()
}

mod word_motion {
    use super::*;

    #[test]
    fn m_f_and_m_b_go_by_japanese_words() {
        let line = "日本語の文章で書く";
        let (mut d, mut editor) = at(line, &[0]);
        assert_eq!(send(&mut d, &mut editor, "M-f"), REDRAW);
        assert_eq!(cursors(&editor), ["日本語の".len()]);
        send(&mut d, &mut editor, "M-f");
        assert_eq!(cursors(&editor), ["日本語の文章で".len()]);
        send(&mut d, &mut editor, "M-f");
        assert_eq!(cursors(&editor), [line.len()]);
        assert_eq!(send(&mut d, &mut editor, "M-f"), NOTHING, "the end");
        send(&mut d, &mut editor, "M-b M-b");
        assert_eq!(cursors(&editor), ["日本語の".len()]);
        send(&mut d, &mut editor, "M-b");
        assert_eq!(send(&mut d, &mut editor, "M-b"), NOTHING, "the start");
        assert_eq!(cursors(&editor), [0]);
    }

    #[test]
    fn counts_and_negative_counts_go_either_way() {
        let (mut d, mut editor) = at("fooBar baz qux", &[0]);
        send(&mut d, &mut editor, "M-3 M-f");
        assert_eq!(cursors(&editor), [10]);
        send(&mut d, &mut editor, "M-- M-f");
        assert_eq!(cursors(&editor), [7]);
        send(&mut d, &mut editor, "C-u - 2 M-b");
        assert_eq!(cursors(&editor), [14]);
        assert_eq!(send(&mut d, &mut editor, "M-0 M-b"), NOTHING);
    }

    #[test]
    fn a_long_vowel_after_hiragana_stays_with_it_through_the_keys() {
        let (mut d, mut editor) = at("すごーい", &[0]);
        send(&mut d, &mut editor, "M-f");
        assert_eq!(cursors(&editor), ["すごー".len()]);
        send(&mut d, &mut editor, "M-d");
        assert_eq!(text(&editor), "すごー");
    }

    #[test]
    fn m_d_stops_after_an_emoji_with_a_skin_tone() {
        let (mut d, mut editor) = at("☝\u{1f3fd} next", &[0]);
        send(&mut d, &mut editor, "M-d");
        assert_eq!(text(&editor), " next");
    }

    #[test]
    fn every_cursor_moves_and_those_that_meet_merge() {
        let (mut d, mut editor) = at("one two three", &[0, 1, 4]);
        send(&mut d, &mut editor, "M-f");
        assert_eq!(cursors(&editor), [3, 7]);
        send(&mut d, &mut editor, "M-b");
        assert_eq!(cursors(&editor), [0, 4]);
    }

    #[test]
    fn m_d_deletes_to_the_end_of_the_word_as_one_undo_step() {
        let (mut d, mut editor) = at("one two three", &[0]);
        assert_eq!(send(&mut d, &mut editor, "M-d"), REDRAW);
        assert_eq!(text(&editor), " two three");
        send(&mut d, &mut editor, "M-d");
        assert_eq!(text(&editor), " three");
        assert_eq!(cursors(&editor), [0]);
        send(&mut d, &mut editor, "C-z");
        assert_eq!(text(&editor), " two three");
        send(&mut d, &mut editor, "C-z");
        assert_eq!(text(&editor), "one two three");
        assert!(!editor.current().1.is_modified());
    }

    #[test]
    fn m_del_deletes_back_to_the_start_of_the_word() {
        let (mut d, mut editor) = at("日本語の文章で", &["日本語の文章で".len()]);
        assert_eq!(send(&mut d, &mut editor, "M-DEL"), REDRAW);
        assert_eq!(text(&editor), "日本語の");
        assert_eq!(cursors(&editor), ["日本語の".len()]);
        send(&mut d, &mut editor, "C-z");
        assert_eq!(text(&editor), "日本語の文章で");
        assert_eq!(cursors(&editor), ["日本語の文章で".len()]);
    }

    #[test]
    fn deleting_words_takes_counts_and_their_signs() {
        let (mut d, mut editor) = at("one two three four", &[8]);
        send(&mut d, &mut editor, "M-2 M-d");
        assert_eq!(text(&editor), "one two ");
        send(&mut d, &mut editor, "C-z M-2 M-DEL");
        assert_eq!(text(&editor), "three four");
        send(&mut d, &mut editor, "C-z M-- M-d");
        assert_eq!(text(&editor), "one three four");
        send(&mut d, &mut editor, "C-z M-- M-DEL");
        assert_eq!(text(&editor), "one two  four");
        send(&mut d, &mut editor, "C-z M-0 M-d");
        assert_eq!(text(&editor), "one two three four");
    }

    #[test]
    fn at_the_edges_deleting_a_word_does_nothing_and_says_nothing() {
        let (mut d, mut editor) = at("ab", &[2]);
        assert_eq!(send(&mut d, &mut editor, "M-d"), NOTHING);
        assert!(kept_messages(&editor).is_empty());
        place(&mut editor, &[0]);
        assert_eq!(send(&mut d, &mut editor, "M-DEL"), NOTHING);
        assert!(kept_messages(&editor).is_empty());
        assert_eq!(text(&editor), "ab");
    }

    #[test]
    fn a_count_past_the_edge_deletes_what_there_is() {
        let (mut d, mut editor) = at("ab cd", &[0]);
        send(&mut d, &mut editor, "M-9 M-d");
        assert_eq!(text(&editor), "");
        send(&mut d, &mut editor, "C-z");
        place(&mut editor, &[5]);
        let dispatched = d.run_command(&mut editor, "backward-kill-word", Some(isize::MIN));
        assert_eq!(dispatched.1, Ok(()));
        assert_eq!(text(&editor), "ab cd", "isize::MIN goes the other way");
    }

    #[test]
    fn every_cursor_deletes_in_one_undo_step_and_overlaps_merge() {
        let (mut d, mut editor) = at("one two three", &[0, 4]);
        send(&mut d, &mut editor, "M-d");
        assert_eq!(text(&editor), "  three");
        assert_eq!(cursors(&editor), [0, 1]);
        send(&mut d, &mut editor, "C-z");
        assert_eq!(text(&editor), "one two three");
        assert_eq!(cursors(&editor), [0, 4]);
        // The second cursor's word holds the first cursor.
        let (mut d, mut editor) = at("one two", &[2, 3]);
        send(&mut d, &mut editor, "M-d");
        assert_eq!(text(&editor), "on");
        assert_eq!(cursors(&editor), [2]);
        send(&mut d, &mut editor, "C-z");
        assert_eq!(text(&editor), "one two");
    }

    #[test]
    fn a_read_only_buffer_refuses_a_deleted_word_even_with_nothing_to_delete() {
        let mut editor = Editor::with_buffer(Buffer::from_bytes("bad", b"ab \xff"));
        editor.set_view_size(ViewSize::new(10, 20));
        let mut d = Dispatcher::new();
        assert_eq!(send(&mut d, &mut editor, "M-d"), NOTHING);
        assert_eq!(kept_messages(&editor).len(), 1);
        send(&mut d, &mut editor, "M-DEL");
        assert_eq!(kept_messages(&editor).len(), 2);
        assert_eq!(editor.current().1.text().len(), 6);
    }

    #[test]
    fn a_count_that_spends_exactly_what_a_command_may_read_still_deletes() {
        // The walk's start and each line break cost a seek, 8192 of them in all.
        let lines = "\n".repeat(8191);
        let (mut d, mut editor) = at(&lines, &[0]);
        send(&mut d, &mut editor, "C-u 8 1 9 1 C-d");
        assert_eq!(text(&editor), "");
        assert!(kept_messages(&editor).is_empty());
        // One more goes past what it may read: nothing, and a signal.
        let lines = "\n".repeat(8193);
        let (mut d, mut editor) = at(&lines, &[0]);
        send(&mut d, &mut editor, "C-u 8 1 9 2 C-d");
        assert_eq!(text(&editor).len(), 8193);
        assert_eq!(
            kept_messages(&editor).last().map(String::as_str),
            Some("Count too large")
        );
    }

    #[test]
    fn a_long_line_with_no_boundary_is_passed_a_piece_at_a_time() {
        let long = "x".repeat(400_000);
        let (mut d, mut editor) = at(&long, &[0]);
        send(&mut d, &mut editor, "M-f");
        let first = cursors(&editor)[0];
        assert!(first > 0 && first < long.len(), "{first}");
        send(&mut d, &mut editor, "M-f M-f M-f M-f");
        assert_eq!(cursors(&editor), [long.len()]);
        send(&mut d, &mut editor, "M-DEL");
        let left = editor.current().1.text().len();
        assert!(left > 0 && left < long.len(), "{left}");
        send(&mut d, &mut editor, "C-z");
        assert_eq!(editor.current().1.text().len(), long.len());
        assert!(
            kept_messages(&editor).is_empty(),
            "one cursor reads a stop a key"
        );
    }

    #[test]
    fn a_thousand_cursors_on_ordinary_text_all_move_by_a_word() {
        // Each cursor's seeks to where it starts come apart from the bytes they share
        // (ADR 0101), so the most cursors a window may have all move.
        let words = "word\n".repeat(1000);
        let ends: Vec<usize> = (0..1000).map(|line| line * 5 + 4).collect();
        let (mut d, mut editor) = at(&words, &ends);
        send(&mut d, &mut editor, "M-b");
        let starts: Vec<usize> = ends.iter().map(|end| end - 4).collect();
        assert_eq!(cursors(&editor), starts);
        send(&mut d, &mut editor, "M-f");
        assert_eq!(cursors(&editor), ends);
        send(&mut d, &mut editor, "M-DEL");
        assert_eq!(text(&editor), "\n".repeat(1000));
        assert!(kept_messages(&editor).is_empty());
    }

    /// Lines with no word boundary, and a cursor at the start of each.
    fn long_runs(lines: usize) -> (String, Vec<usize>) {
        let line = format!("{}\n", "x".repeat(200_000));
        let starts = (0..lines).map(|index| index * line.len()).collect();
        (line.repeat(lines), starts)
    }

    #[test]
    fn cursors_on_long_runs_share_one_commands_reading() {
        // Each stop reads as much as it may, and the cursors share what one command at
        // several reads (ADR 0101): the first move, and those it runs out on stay.
        let (long, starts) = long_runs(8);
        let (mut d, mut editor) = at(&long, &starts);
        send(&mut d, &mut editor, "M-f");
        let moved: Vec<bool> = cursors(&editor)
            .iter()
            .zip(&starts)
            .map(|(cursor, start)| cursor > start)
            .collect();
        assert!(moved[0] && !moved[7], "{moved:?}");
        assert!(moved.windows(2).all(|pair| pair[0] || !pair[1]), "in order");
        assert_eq!(
            kept_messages(&editor).last().map(String::as_str),
            Some("Count too large")
        );

        let (mut d, mut editor) = at(&long, &starts);
        send(&mut d, &mut editor, "M-d");
        let left = text(&editor);
        let lines: Vec<usize> = left.split('\n').map(str::len).collect();
        assert!(lines[0] < 200_000 && lines[7] == 200_000, "{lines:?}");
        assert_eq!(
            kept_messages(&editor).last().map(String::as_str),
            Some("Count too large")
        );
    }
}

mod mwim {
    use super::*;

    #[test]
    fn c_a_and_c_e_toggle_through_their_keys() {
        let line = "  code  ";
        let (mut d, mut editor) = at(line, &[4]);
        assert_eq!(send(&mut d, &mut editor, "C-a"), REDRAW);
        assert_eq!(cursors(&editor), [2]);
        send(&mut d, &mut editor, "C-a");
        assert_eq!(cursors(&editor), [0]);
        send(&mut d, &mut editor, "C-a");
        assert_eq!(cursors(&editor), [2]);
        send(&mut d, &mut editor, "C-e");
        assert_eq!(cursors(&editor), [6]);
        send(&mut d, &mut editor, "C-e");
        assert_eq!(cursors(&editor), [8]);
        send(&mut d, &mut editor, "C-e");
        assert_eq!(cursors(&editor), [6]);
    }

    #[test]
    fn home_and_end_go_to_the_ends_of_the_line() {
        let (mut d, mut editor) = at("  code  ", &[4]);
        send(&mut d, &mut editor, "<home>");
        assert_eq!(cursors(&editor), [0]);
        send(&mut d, &mut editor, "<end>");
        assert_eq!(cursors(&editor), [8]);
    }

    #[test]
    fn each_cursor_toggles_on_its_own_line() {
        let (mut d, mut editor) = at("  a  \n  b  \n", &[2, 8]);
        send(&mut d, &mut editor, "C-a");
        assert_eq!(cursors(&editor), [0, 6]);
        send(&mut d, &mut editor, "C-a C-e");
        assert_eq!(cursors(&editor), [3, 9]);
    }

    #[test]
    fn nothing_to_move_draws_nothing() {
        let (mut d, mut editor) = at("", &[0]);
        assert_eq!(send(&mut d, &mut editor, "C-a"), NOTHING);
        assert_eq!(send(&mut d, &mut editor, "C-e"), NOTHING);
    }

    #[test]
    fn a_count_goes_through_their_keys() {
        let (mut d, mut editor) = at("a\n  b  \nc", &[0]);
        send(&mut d, &mut editor, "C-u 2 C-a");
        assert_eq!(cursors(&editor), [4]);
        send(&mut d, &mut editor, "M-0 C-e");
        assert_eq!(cursors(&editor), [1]);
    }

    #[test]
    fn line_motion_stays_at_the_line_end_after_the_second_c_e_only() {
        let text = "ab  \nabcdefgh\n";
        let (mut d, mut editor) = at(text, &[0]);
        send(&mut d, &mut editor, "C-e C-n");
        assert_eq!(cursors(&editor), [5 + 2], "the same column");
        let (mut d, mut editor) = at(text, &[0]);
        send(&mut d, &mut editor, "C-e C-e C-n");
        assert_eq!(cursors(&editor), [13], "the next line's end");
    }
}

mod recenter {
    use super::*;

    /// A dispatcher and an editor of 100 numbered lines, in a window of 10 rows.
    fn numbered(row: usize) -> (Dispatcher, Editor) {
        let lines: String = (0..100).map(|i| format!("{i:02}\n")).collect();
        let (mut d, mut editor) = at(&lines, &[row * 3]);
        send(&mut d, &mut editor, "C-f C-b");
        (d, editor)
    }

    fn top(editor: &Editor) -> usize {
        editor.top_row()
    }

    #[test]
    fn c_l_cycles_middle_top_bottom() {
        let (mut d, mut editor) = numbered(50);
        assert_eq!(send(&mut d, &mut editor, "C-l"), REDRAW);
        assert_eq!(top(&editor), 45);
        send(&mut d, &mut editor, "C-l");
        assert_eq!(top(&editor), 50);
        send(&mut d, &mut editor, "C-l");
        assert_eq!(top(&editor), 41);
        send(&mut d, &mut editor, "C-l");
        assert_eq!(top(&editor), 45, "round again");
        assert_eq!(cursors(&editor), [150], "the cursor stays");
    }

    #[test]
    fn another_key_starts_the_cycle_over() {
        let (mut d, mut editor) = numbered(50);
        send(&mut d, &mut editor, "C-l C-f C-b C-l");
        assert_eq!(top(&editor), 45);
        send(&mut d, &mut editor, "C-l <f35> C-l");
        assert_eq!(top(&editor), 45, "an undefined key too");
    }

    #[test]
    fn a_count_puts_the_cursor_on_that_row() {
        let (mut d, mut editor) = numbered(50);
        send(&mut d, &mut editor, "M-3 C-l");
        assert_eq!(top(&editor), 47);
        send(&mut d, &mut editor, "M-0 C-l");
        assert_eq!(top(&editor), 50);
        send(&mut d, &mut editor, "M-- C-l");
        assert_eq!(top(&editor), 41);
        send(&mut d, &mut editor, "M-- 2 C-l");
        assert_eq!(top(&editor), 42);
        send(&mut d, &mut editor, "C-u C-l");
        assert_eq!(top(&editor), 45, "a plain C-u: the middle");
    }

    #[test]
    fn a_minus_alone_scrolls_a_screen_the_other_way() {
        let (mut d, mut editor) = numbered(50);
        let before = top(&editor);
        send(&mut d, &mut editor, "M-- C-v");
        assert_eq!(top(&editor), before - 8);
        send(&mut d, &mut editor, "M-- M-v");
        assert_eq!(top(&editor), before);
    }

    #[test]
    fn near_the_start_the_view_stays_and_draws_nothing() {
        let (mut d, mut editor) = numbered(2);
        assert_eq!(send(&mut d, &mut editor, "C-l"), NOTHING);
        assert_eq!(top(&editor), 0);
        assert_eq!(send(&mut d, &mut editor, "C-l"), REDRAW, "the top");
        assert_eq!(top(&editor), 2);
        assert_eq!(send(&mut d, &mut editor, "C-l"), REDRAW, "the bottom");
        assert_eq!(top(&editor), 0);
    }

    #[test]
    fn other_cursors_in_view_stay_and_those_left_behind_move() {
        let lines: String = (0..100).map(|i| format!("{i:02}\n")).collect();
        let (mut d, mut editor) = at(&lines, &[50 * 3, 54 * 3 + 1, 90 * 3 + 1]);
        send(&mut d, &mut editor, "C-f C-b");
        send(&mut d, &mut editor, "C-l");
        assert_eq!(top(&editor), 45);
        assert_eq!(cursors(&editor), [150, 54 * 3, 54 * 3 + 1]);
    }
}

mod wheel {
    use super::*;

    #[test]
    fn a_wheel_with_the_view_sideways_brings_a_moved_cursor_back_into_view() {
        let long = "x".repeat(60);
        let lines: String = (0..40).map(|_| format!("{long}\n")).collect();
        let (mut d, mut editor) = at(&lines, &[0]);
        send(&mut d, &mut editor, "C-e");
        assert_eq!(editor.window().scroll().left, 41);
        assert!(editor.scroll_view(editor.tiled().0.id(), 20));
        assert_eq!((editor.top_row(), editor.window().scroll().left), (20, 0));
        assert_eq!(cursors(&editor), [20 * 61], "at column 0, in view");
    }

    #[test]
    fn the_sideways_wheel_leaves_the_cursor_and_the_next_key_comes_back() {
        let long = "x".repeat(60);
        let (mut d, mut editor) = at(&long, &[0]);
        assert!(editor.scroll_view_sideways(editor.tiled().0.id(), 30));
        assert_eq!(editor.window().scroll().left, 30);
        assert_eq!(cursors(&editor), [0]);
        assert_eq!(send(&mut d, &mut editor, "<f35>"), NOTHING, "nothing ran");
        assert_eq!(editor.window().scroll().left, 30);
        assert_eq!(send(&mut d, &mut editor, "C-f"), REDRAW);
        assert_eq!(editor.window().scroll().left, 1);
    }

    #[test]
    fn scrolling_down_keeps_the_view_sideways_while_the_cursor_is_in_view() {
        let long = "x".repeat(60);
        let lines: String = (0..40).map(|_| format!("{long}\n")).collect();
        let (mut d, mut editor) = at(&lines, &[61 * 5]);
        send(&mut d, &mut editor, "C-f C-b");
        assert!(editor.scroll_view_sideways(editor.tiled().0.id(), 10));
        assert!(editor.scroll_view(editor.tiled().0.id(), 2));
        assert_eq!((editor.top_row(), editor.window().scroll().left), (2, 10));
    }
}

/// `C-e` stops before a trailing comment as highlighting says (ADR 0111): every cursor at its
/// own line's code end, and with font-lock off, where the text ends.
#[test]
fn c_e_stops_before_a_trailing_comment_for_every_cursor() {
    use oso_core::buffer::Mode;
    use oso_core::syntax::{Highlight, Highlights};
    let text = "f(); // c\nlet s = \"// no\";\n";
    let (mut dispatcher, mut editor) = at(text, &[0, 10]);
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    assert!(buffer.set_mode(Mode::Rust));
    let mut highlights = Highlights::new(text.len());
    highlights.replace(5..9, [(4, Some(Highlight::Comment))]);
    highlights.replace(18..25, [(7, Some(Highlight::String))]);
    let revision = buffer.revision();
    assert!(buffer.take_highlights(revision, highlights));
    send(&mut dispatcher, &mut editor, "C-e");
    assert_eq!(cursors(&editor), [4, 26]);
    send(&mut dispatcher, &mut editor, "C-e");
    assert_eq!(cursors(&editor), [9, 26]);
    editor.set_font_lock(false);
    place_at(&mut editor, &[0]);
    send(&mut dispatcher, &mut editor, "C-e");
    assert_eq!(cursors(&editor), [9]);
}
