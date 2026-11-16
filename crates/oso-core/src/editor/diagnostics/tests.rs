//! Tests of the cells a buffer's diagnostics take in a window, and of the gutter's signs
//! (ADR 0138, step 7).
//!
//! Must not depend on: `command`, `keymap` or the host's window.

use std::ops::Range;

use super::*;
use crate::buffer::{Buffer, ServerMark};
use crate::diagnostic::{New, Tags};
use crate::editor::{Gutter, VisualAids};
use crate::window::ViewSize;

/// An editor of one window of `rows` by `columns` on `text`, which a server follows.
fn followed(text: &str, (rows, columns): (usize, usize)) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", text));
    editor.set_view_size(ViewSize::new(rows, columns));
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.set_followed(true);
    buffer.set_server(Some(ServerMark::Ready));
    editor.fit_gutter();
    editor
}

/// Reports `entries` for the text as it is: each a range, a severity and its tags.
fn report(editor: &mut Editor, entries: &[(Range<usize>, Severity, Tags)]) {
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    let new = entries.iter().map(|(range, severity, tags)| New {
        range: range.clone(),
        severity: *severity,
        tags: *tags,
        source: "",
        code: "",
        message: "m",
    });
    let set = Diagnostics::new("server", new, buffer.text().len());
    let revision = buffer.revision();
    assert!(buffer.take_diagnostics(revision, set));
}

const PLAIN: Tags = Tags {
    unnecessary: false,
    deprecated: false,
};
const UNUSED: Tags = Tags {
    unnecessary: true,
    deprecated: false,
};
const OLD: Tags = Tags {
    unnecessary: false,
    deprecated: true,
};

/// The cells of each list, as (row, column, columns), the text area's origin taken off.
type Shown = [Vec<(usize, usize, usize)>; 5];

fn cells(editor: &Editor) -> Shown {
    let mut cells = DiagnosticCells::default();
    let id = editor.tiled().0.id();
    editor.diagnostic_cells(id, &editor.screen_now(), &mut cells);
    let text = editor.tiled_regions().text;
    let shown = |list: &Vec<CellRect>| {
        list.iter()
            .map(|cell| {
                assert_eq!(cell.rows, 1);
                (cell.row - text.row, cell.column - text.column, cell.columns)
            })
            .collect()
    };
    [
        shown(&cells.error),
        shown(&cells.warning),
        shown(&cells.information),
        shown(&cells.unnecessary),
        shown(&cells.deprecated),
    ]
}

#[test]
fn each_severity_has_its_cells_and_the_most_severe_wins_where_they_meet() {
    let mut editor = followed("let x: u8 = 300;\nfn f() {}", (5, 40));
    report(
        &mut editor,
        &[
            // A warning over `x: u8 = 300`, an error over `300`, information over `fn f`, and
            // a hint that draws nothing.
            (4..15, Severity::Warning, PLAIN),
            (12..15, Severity::Error, PLAIN),
            (17..21, Severity::Information, PLAIN),
            (0..3, Severity::Hint, PLAIN),
        ],
    );
    let [error, warning, information, unnecessary, deprecated] = cells(&editor);
    assert_eq!(error, [(0, 12, 3)]);
    assert_eq!(
        warning,
        [(0, 4, 8)],
        "the warning's line stops where the error's starts"
    );
    assert_eq!(information, [(1, 0, 4)]);
    assert!(unnecessary.is_empty() && deprecated.is_empty());
}

#[test]
fn a_range_stops_at_its_lines_text_and_an_empty_one_takes_its_cell() {
    let mut editor = followed("ab\ncd\nef", (5, 40));
    report(
        &mut editor,
        &[
            // From `b` into the next line: to `b`'s end, then `c`.
            (1..4, Severity::Error, PLAIN),
            // Empty at `d`'s line's end: its line break's cell.
            (5..5, Severity::Warning, PLAIN),
            // The second line break alone: its cell too, as the error's line does not reach it.
            (5..6, Severity::Information, PLAIN),
        ],
    );
    let [error, warning, information, ..] = cells(&editor);
    assert_eq!(error, [(0, 1, 1), (1, 0, 1)]);
    assert_eq!(warning, [(1, 2, 1)]);
    assert!(
        information.is_empty(),
        "under the warning's cell: {information:?}"
    );
}

#[test]
fn faded_and_struck_cells_follow_the_tags_whatever_the_severity() {
    let mut editor = followed("use a;\nold();", (5, 40));
    report(
        &mut editor,
        &[
            // An unused import as a hint: faded, with no line.
            (0..6, Severity::Hint, UNUSED),
            (7..10, Severity::Warning, OLD),
        ],
    );
    let [error, warning, _, unnecessary, deprecated] = cells(&editor);
    assert!(error.is_empty());
    assert_eq!(warning, [(1, 0, 3)]);
    assert_eq!(unnecessary, [(0, 0, 6)]);
    assert_eq!(deprecated, [(1, 0, 3)]);
}

#[test]
fn a_wrapped_line_has_the_cells_of_each_of_its_rows() {
    // Twenty columns of text beside the sign column: the line wraps after its fourth word.
    let text = "word word word word word word";
    let mut editor = followed(text, (5, 21));
    let id = editor.window().buffer();
    assert!(editor.set_wraps(id, true));
    crate::editor::wrap_tests::settle(&mut editor);
    // The fourth word, the blank that hangs in the wrap mark's column, and the fifth's `wor`.
    // No line goes under the mark.
    report(&mut editor, &[(15..23, Severity::Error, PLAIN)]);
    let [error, ..] = cells(&editor);
    assert_eq!(error, [(0, 15, 4), (1, 0, 3)]);
}

#[test]
fn a_buffer_no_server_follows_has_no_cells_and_no_sign_column() {
    let mut editor = followed("abc", (5, 40));
    report(&mut editor, &[(0..1, Severity::Error, PLAIN)]);
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.set_followed(false);
    buffer.set_server(None);
    editor.fit_gutter();
    assert!(cells(&editor).iter().all(Vec::is_empty));
    assert_eq!(editor.tiled_regions().gutter.columns, 0);
}

fn gutter(editor: &Editor) -> Gutter {
    let mut gutter = Gutter::default();
    editor.gutter_into(editor.tiled().0.id(), &editor.screen_now(), &mut gutter);
    gutter
}

#[test]
fn the_sign_column_is_there_before_a_report_and_beside_line_numbers() {
    let mut editor = followed("a\nb", (3, 20));
    assert_eq!(
        editor.tiled_regions().gutter.columns,
        1,
        "before any report"
    );
    assert_eq!(gutter(&editor).signs(), [None, None]);
    assert!(gutter(&editor).rows().all(|(text, _)| text.is_empty()));
    editor.set_visual_aids(VisualAids {
        line_numbers: true,
        ..VisualAids::default()
    });
    assert_eq!(
        editor.tiled_regions().gutter.columns,
        5,
        "a sign and four for numbers"
    );
    let numbers: Vec<_> = gutter(&editor)
        .rows()
        .map(|(text, _)| text.to_owned())
        .collect();
    assert_eq!(numbers, ["  1 ", "  2 "]);
}

#[test]
fn a_sign_is_the_most_severe_that_starts_on_its_line_hints_left_out() {
    let mut editor = followed("aa\nbb\ncc\ndd", (4, 20));
    report(
        &mut editor,
        &[
            // Starts on line 1 and runs into line 2, which gets no sign of it.
            (1..7, Severity::Warning, PLAIN),
            (0..1, Severity::Information, PLAIN),
            (4..5, Severity::Error, PLAIN),
            (6..7, Severity::Hint, PLAIN),
            // Empty at line 4's end.
            (11..11, Severity::Warning, PLAIN),
        ],
    );
    let (e, w) = (Some(Severity::Error), Some(Severity::Warning));
    assert_eq!(gutter(&editor).signs(), [w, e, None, w]);
}

#[test]
fn a_sign_is_on_a_lines_first_row_only() {
    let mut editor = followed("word word word word word word\nk", (4, 21));
    let id = editor.window().buffer();
    assert!(editor.set_wraps(id, true));
    crate::editor::wrap_tests::settle(&mut editor);
    // On the line's second row.
    report(&mut editor, &[(22..23, Severity::Error, PLAIN)]);
    let signs = gutter(&editor).signs().to_vec();
    assert_eq!(signs, [Some(Severity::Error), None, None]);
}

#[test]
fn a_range_that_starts_above_the_view_has_its_cells_and_no_sign_in_it() {
    let text: String = (0..30).map(|n| format!("line {n:02}\n")).collect();
    let mut editor = followed(&text, (4, 20));
    // From line 1 to line 12, and a warning that starts on line 11.
    report(
        &mut editor,
        &[
            (9..100, Severity::Error, PLAIN),
            (90..91, Severity::Warning, PLAIN),
        ],
    );
    let id = editor.tiled().0.id();
    assert!(editor.scroll_view(id, 10));
    let [error, warning, ..] = cells(&editor);
    // Lines 10 and 11 to their text's end, then line 12's `line`, then nothing.
    assert_eq!(error, [(0, 0, 7), (1, 0, 7), (2, 0, 4)]);
    assert!(warning.is_empty(), "under the error's line");
    let signs = gutter(&editor).signs().to_vec();
    assert_eq!(signs, [None, Some(Severity::Warning), None, None]);
}
