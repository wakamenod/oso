//! Text-grid snapshots (ADR 0037, layer 3; ADR 0045): key sequences run
//! through the [`Dispatcher`], then the screen is compared as text.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on
//! every OS with no window.
//!
//! The tests live here because `oso` is a binary crate, so `tests/` cannot
//! reach the `Dispatcher`. The grid draws through the code the renderer uses
//! (`DisplayCluster::for_each_run`, `Window::cursor_point`,
//! `motion::cursor_cells`, `frame::cursor_rect`), so a snapshot fails when
//! the cells on screen change. Pixels are not covered (ADR 0037, layer 4).
//!
//! A frame is its regions from the top (ADR 0057). Each row of the text area is
//! one line between `│`, the modeline's between `=` and the echo area's between
//! `"`. Below a row that needs it, a line between `:` marks its cells. See
//! `docs/dev/testing.md`.

use oso_core::buffer::Buffer;
use std::ops::Range;

use oso_core::display_map::{CellRun, CellStyle, DisplayPoint, DisplaySnapshot};
use oso_core::editor::Editor;
use oso_core::frame::{self, CellRect, FrameSize};
use oso_core::key::KeySequence;
use oso_core::motion;
use oso_core::window::{Scroll, ViewSize};

use super::Dispatcher;

/// The left half of a wide cluster cut off by the right edge of the window.
const LEFT_HALF: char = '▌';
/// The right half of a wide cluster cut off by the left edge of the window.
const RIGHT_HALF: char = '▐';

/// The frame's screen: its regions from the top, one line per row and one per
/// marked row.
fn text_grid(editor: &Editor) -> String {
    let regions = editor.frame();
    let mut lines = Vec::new();
    draw_text_area(editor, regions.window.text, &mut lines);
    draw_blank(regions.window.modeline, '=', &mut lines);
    draw_blank(regions.echo, '"', &mut lines);
    lines.join("\n")
}

/// The selected window's rows.
fn draw_text_area(editor: &Editor, area: CellRect, lines: &mut Vec<String>) {
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let scroll = window.scroll();
    let point = window.cursor_point(&display);
    // The cursor box as the renderer draws it.
    let cursor = frame::cursor_rect(area, scroll, point, motion::cursor_cells(&display, point));
    for index in 0..area.rows {
        let row = scroll.top + index;
        let cursor = cursor
            .filter(|cells| cells.row == area.row + index)
            .map_or(0..0, |cells| {
                let start = cells.column - area.column;
                start..start + cells.columns
            });
        let (text, marks) = draw_row(&display, row, scroll, area.columns, cursor);
        lines.push(format!("│{text}│"));
        if marks.iter().any(|&mark| mark != ' ') {
            let marks: String = marks.into_iter().collect();
            lines.push(format!(":{marks}:"));
        }
    }
}

/// A region with nothing to show yet, between `edge`s.
fn draw_blank(area: CellRect, edge: char, lines: &mut Vec<String>) {
    for _ in 0..area.rows {
        lines.push(format!("{edge}{}{edge}", " ".repeat(area.columns)));
    }
}

/// The text of one row of `columns` cells and a mark for each of them. `cursor`
/// holds the cells under the cursor box, counted from the row's left edge.
fn draw_row(
    display: &DisplaySnapshot,
    row: usize,
    scroll: Scroll,
    columns: usize,
    cursor: Range<usize>,
) -> (String, Vec<char>) {
    let view = scroll.left..scroll.left + columns;
    let mut text = String::new();
    let mut marks = vec![' '; columns];
    let mut drawn = view.start;
    for cluster in display.clusters(DisplayPoint::new(row, view.start)) {
        if cluster.column >= view.end {
            break;
        }
        cluster.for_each_run(|run| {
            let start = run.column.max(view.start);
            let end = (run.column + run.cells).min(view.end);
            if start < end {
                let cells = &mut marks[start - view.start..end - view.start];
                draw_run(&run, start, cells, &mut text);
                drawn = end;
            }
        });
    }
    text.extend(std::iter::repeat_n(' ', view.end - drawn));
    marks[cursor].fill('^');
    (text, marks)
}

/// Writes the visible part of `run`, which starts on screen at column `start`
/// and takes the cells of `marks`.
fn draw_run(run: &CellRun<'_>, start: usize, marks: &mut [char], text: &mut String) {
    if run.text == " " {
        text.extend(std::iter::repeat_n(' ', marks.len()));
    } else if run.cells == marks.len() {
        text.push_str(run.text);
        match (run.style, &mut *marks) {
            (CellStyle::Text, [first, second]) => (*first, *second) = ('<', '>'),
            (CellStyle::Escape, _) => marks.fill('~'),
            (CellStyle::Boxed, _) => marks.fill('#'),
            (CellStyle::Text, _) => {}
        }
    } else if start == run.column {
        text.push(LEFT_HALF);
    } else {
        text.push(RIGHT_HALF);
    }
}

/// How `bytes` reads in a snapshot's description: no raw invisible characters
/// or invalid bytes.
fn describe(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.utf8_chunks() {
        out.extend(chunk.valid().chars().flat_map(char::escape_debug));
        for byte in chunk.invalid() {
            out.push_str(&format!("\\x{byte:02X}"));
        }
    }
    out
}

/// An editor showing `bytes` in a window of `rows` by `columns`.
fn open(bytes: &[u8], (rows, columns): (usize, usize)) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_bytes("test", bytes));
    editor.set_view_size(ViewSize::new(rows, columns));
    editor
}

/// Compares `frames` of a scenario on `bytes` with the snapshot `name`.
fn assert_frames(name: &str, bytes: &[u8], frames: &[String]) {
    let mut settings = insta::Settings::clone_current();
    settings.set_prepend_module_to_snapshot(false);
    settings.set_omit_expression(true);
    settings.set_description(format!("text: {}", describe(bytes)));
    settings.bind(|| insta::assert_snapshot!(name, frames.join("\n\n")));
}

/// Opens `text` in a window of `size` (rows, columns) and checks one frame per
/// entry of `steps`: the keys to press first, on top of the earlier ones.
fn check(name: &str, text: impl AsRef<[u8]>, size: (usize, usize), steps: &[&str]) {
    check_bytes(name, text.as_ref(), size, steps);
}

fn check_bytes(name: &str, bytes: &[u8], size: (usize, usize), steps: &[&str]) {
    check_editor(name, bytes, open(bytes, size), steps);
}

/// As [`check`], with the whole frame given in rows and columns, so that the rows
/// it reserves come out of them.
fn check_frame(name: &str, text: &str, (rows, columns): (usize, usize), steps: &[&str]) {
    let mut editor = Editor::with_buffer(Buffer::from_bytes("test", text.as_bytes()));
    editor.set_frame_size(FrameSize { rows, columns });
    check_editor(name, text.as_bytes(), editor, steps);
}

fn check_editor(name: &str, bytes: &[u8], mut editor: Editor, steps: &[&str]) {
    let mut dispatcher = Dispatcher::new();
    let mut frames = Vec::new();
    for &keys in steps {
        if !keys.is_empty() {
            let sequence: KeySequence = keys.parse().unwrap();
            dispatcher.handle_keys(&mut editor, sequence.0);
            assert!(dispatcher.pending().is_empty(), "{keys} left a prefix");
        }
        let keys = if keys.is_empty() { "none" } else { keys };
        frames.push(format!("keys: {keys}\n{}", text_grid(&editor)));
    }
    assert_frames(name, bytes, &frames);
}

// `cfg(test)` again so that the size limit counts the grid and its helpers, not
// the scenarios, which grow with every visible feature (ADR 0045).
#[cfg(test)]
mod scenarios {
    use super::*;

    #[test]
    fn full_width_and_half_width() {
        check(
            "widths_japanese",
            "日本語abc\nｱｲｳ半角\nＡＢＣ全角\n",
            (4, 16),
            &[""],
        );
    }

    #[test]
    fn emoji_and_combining_marks_take_their_cluster_width() {
        let text = "👨\u{200d}👩\u{200d}👧|family\n🇯🇵|flag\n🍣|sushi\n1\u{fe0f}\u{20e3}|keycap\n\
                    か\u{3099}|voiced\ne\u{301}|acute\n\u{301}|lone mark";
        check("widths_emoji_and_marks", text, (7, 16), &[""]);
    }

    #[test]
    fn tabs_reach_the_next_stop() {
        let text = "a\tb\n\tc\n日本\td\n1234567\te\n12345678\tf";
        check("tabs", text, (5, 24), &["", "C-n C-f C-f"]);
    }

    #[test]
    fn line_breaks_and_lone_carriage_returns() {
        check(
            "line_endings",
            "crlf\r\nlone\rcr\nlf\r\n\r\nend\r",
            (6, 12),
            &["", "C-n C-f C-f C-f C-f"],
        );
    }

    #[test]
    fn control_characters_are_escapes() {
        check(
            "controls",
            "bell\x07 del\x7f nul\0\nc1\u{85} esc\x1b[31m",
            (2, 24),
            &[""],
        );
    }

    #[test]
    fn invisible_and_bidi_characters_are_boxed() {
        let text =
            "ok\u{202e}evil\nz\u{200b}w\u{200d}j\n\u{2066}iso\u{2069}\nE\u{e0041}tag\na\u{feff}b";
        check("invisible_and_bidi", text, (5, 24), &[""]);
    }

    #[test]
    fn characters_past_a_clusters_limits_are_boxed() {
        let at_limit = format!("e{}", "\u{301}".repeat(8));
        let past_limit = format!("o{}", "\u{301}".repeat(10));
        let text = format!("{at_limit}x\n{past_limit}x\n");
        let steps = ["", "C-n C-f", "C-f", "C-f", "C-f"];
        check("past_cluster_limits", &text, (3, 24), &steps);
    }

    #[test]
    fn invalid_bytes_are_escapes() {
        check(
            "invalid_bytes",
            b"a\xFFb\n\xE3\x81x\n\xC0\xAF\n",
            (4, 16),
            &["", "C-f"],
        );
    }

    #[test]
    fn escapes_differ_from_text_that_reads_the_same() {
        check(
            "escapes_and_lookalikes",
            b"^M \\xFF U+202E\n\r \xFF \xE2\x80\xAE\n".as_slice(),
            (2, 20),
            &[""],
        );
    }

    #[test]
    fn the_cursor_box_covers_its_whole_cluster() {
        let text = "日a👨\u{200d}👩\u{200d}👧🇯🇵か\u{3099}\tb\r\u{202e}";
        let steps = [
            "", "C-f", "C-f", "C-f", "C-f", "C-f", "C-f", "C-f", "C-f", "C-f",
        ];
        check("cursor_box", text, (1, 32), &steps);
        check("cursor_on_invalid_byte", b"a\xFF", (1, 8), &["", "C-f"]);
    }

    #[test]
    fn page_keys_scroll_by_a_screen() {
        let text: String = (0..12).map(|i| format!("line {i:02}\n")).collect();
        let steps = ["", "C-v", "C-v", "C-v", "M-v", "M-v", "M-v", "M->", "M-<"];
        check("page_scrolling", text, (5, 12), &steps);
    }

    #[test]
    fn the_cursor_and_the_view_stop_above_the_reserved_rows() {
        let text: String = (0..12).map(|i| format!("line {i:02}\n")).collect();
        let steps = ["", "C-n C-n C-n", "C-n", "C-v", "M->", "M-<"];
        check_frame("frame_reserved_rows", &text, (6, 12), &steps);
    }

    #[test]
    fn the_wheel_can_leave_the_cursor_on_a_cluster_cut_at_the_left_edge() {
        let text = "0123456789\n日本語\n";
        let mut dispatcher = Dispatcher::new();
        let mut editor = open(text.as_bytes(), (1, 10));
        dispatcher.handle_keys(&mut editor, "C-e".parse::<KeySequence>().unwrap().0);
        let mut frames = vec![format!("keys: C-e\n{}", text_grid(&editor))];
        editor.scroll_view(1);
        frames.push(format!("wheel: 1\n{}", text_grid(&editor)));
        assert_frames("cursor_cut_at_the_left_edge", text.as_bytes(), &frames);
    }

    #[test]
    fn a_small_frame_gives_up_the_reserved_rows_before_the_text() {
        let text = "one\ntwo\nthree\n";
        check_frame("frame_of_3_rows", text, (3, 8), &["", "C-n", "M->"]);
        check_frame("frame_of_2_rows", text, (2, 8), &["", "C-n"]);
        check_frame("frame_of_1_row", text, (1, 8), &["", "C-n"]);
    }

    #[test]
    fn the_view_follows_the_cursor_by_the_fewest_rows() {
        let text: String = (0..8).map(|i| format!("line {i}\n")).collect();
        check(
            "follow_rows",
            text,
            (3, 8),
            &["", "C-n C-n", "C-n", "C-p C-p C-p"],
        );
    }

    #[test]
    fn a_long_line_scrolls_sideways() {
        let text = "0123456789abcdefghijklmnopqrstuvwxyz\nshort\n";
        check(
            "horizontal_scrolling",
            text,
            (3, 12),
            &["", "C-e", "C-b C-b", "C-a", "C-e C-n", "C-p C-e"],
        );
    }

    #[test]
    fn edges_cut_wide_clusters_escapes_and_tabs() {
        let wide = format!("a{}", "あ".repeat(20));
        check("edges_wide", wide, (1, 10), &["", "C-e"]);
        check("edges_label", "\u{202e}\u{202e}", (1, 8), &["", "C-e"]);
        check("edges_tab", "abc\tdef", (1, 5), &["", "C-e"]);
        check(
            "edges_narrower_than_a_cluster",
            "日本",
            (1, 1),
            &["", "C-f"],
        );
    }

    #[test]
    fn an_empty_buffer_shows_only_the_cursor() {
        check("empty_buffer", "", (3, 8), &[""]);
    }

    #[test]
    fn typing_draws_what_was_typed_with_the_cursor_after_it() {
        let steps = [
            "",
            "a b SPC c",
            "日 本",
            "RET 🍣",
            "DEL",
            "C-a TAB",
            "C-z C-z",
            "C-M-z",
        ];
        check("typing", "", (3, 16), &steps);
    }

    #[test]
    fn typing_in_a_line_pushes_its_text_and_a_line_break_its_rows() {
        let steps = ["", "C-f x", "RET", "C-d", "C-z", "C-z", "C-z"];
        check("typing_in_a_line", "ab\ncd\n", (4, 8), &steps);
    }

    #[test]
    fn a_crlf_file_breaks_lines_as_it_does() {
        let steps = ["", "C-f RET", "DEL"];
        check("typing_crlf", "ab\r\ncd\r\n", (3, 8), &steps);
    }

    #[test]
    fn a_typed_mark_joins_its_cluster_and_del_takes_them_together() {
        let steps = ["", "か", "\u{3099}", "x", "DEL", "DEL"];
        check("typing_marks", "", (1, 8), &steps);
    }

    #[test]
    fn typing_past_the_edge_scrolls_sideways() {
        let steps = ["", "a b c d e f", "語", "DEL DEL DEL DEL DEL"];
        check("typing_scrolls_sideways", "012", (1, 8), &steps);
    }

    #[test]
    fn the_wheel_moves_the_view_and_takes_the_cursor_along() {
        let text: String = (0..8).map(|i| format!("line {i}\n")).collect();
        let mut editor = open(text.as_bytes(), (3, 8));
        let mut frames = vec![format!("wheel: none\n{}", text_grid(&editor))];
        for rows in [2, 100, -1, -100] {
            editor.scroll_view(rows);
            frames.push(format!("wheel: {rows}\n{}", text_grid(&editor)));
        }
        assert_frames("wheel", text.as_bytes(), &frames);
    }
}
