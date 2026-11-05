//! Text-grid snapshots (ADR 0037, layer 3; ADR 0045): key sequences run
//! through the [`Dispatcher`], then the screen is compared as text.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on
//! every OS with no window.
//!
//! The tests live here because `oso` is a binary crate, so `tests/` cannot
//! reach the `Dispatcher`. The grid draws through the code the renderer uses
//! (`DisplayCluster::for_each_run`, `Window::cursor_point`,
//! `motion::cursor_cells`), so a snapshot fails when
//! the cells on screen change. Pixels are not covered (ADR 0037, layer 4).
//!
//! Each row of the window is one line between `│`. Below a row that needs
//! it, a line between `:` marks its cells. See `docs/dev/testing.md`.

use oso_core::buffer::Buffer;
use oso_core::display_map::{CellRun, CellStyle, DisplayPoint, DisplaySnapshot};
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::motion;
use oso_core::window::{Scroll, ViewSize};

use super::Dispatcher;

/// The left half of a wide cluster cut off by the right edge of the window.
const LEFT_HALF: char = '▌';
/// The right half of a wide cluster cut off by the left edge of the window.
const RIGHT_HALF: char = '▐';

/// The cursor as the renderer draws it: where, and how many cells.
type CursorBox = (DisplayPoint, usize);

/// The selected window's screen, one line per row and one per marked row.
fn text_grid(editor: &Editor) -> String {
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let cursor = window.cursor_point(&display);
    let cursor = (cursor, motion::cursor_cells(&display, cursor));
    let mut lines = Vec::new();
    for row in window.scroll().top..window.scroll().top + window.size().rows {
        let (text, marks) = draw_row(&display, row, window.scroll(), window.size(), cursor);
        lines.push(format!("│{text}│"));
        if marks.iter().any(|&mark| mark != ' ') {
            let marks: String = marks.into_iter().collect();
            lines.push(format!(":{marks}:"));
        }
    }
    lines.join("\n")
}

/// The text of one row of the window and a mark for each of its cells.
fn draw_row(
    display: &DisplaySnapshot,
    row: usize,
    scroll: Scroll,
    size: ViewSize,
    cursor: CursorBox,
) -> (String, Vec<char>) {
    let view = scroll.left..scroll.left + size.columns;
    let mut text = String::new();
    let mut marks = vec![' '; size.columns];
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
    let (point, cells) = cursor;
    if point.row == row {
        for column in (point.column..point.column + cells).filter(|c| view.contains(c)) {
            marks[column - view.start] = '^';
        }
    }
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
    let mut dispatcher = Dispatcher::new();
    let mut editor = open(bytes, size);
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
