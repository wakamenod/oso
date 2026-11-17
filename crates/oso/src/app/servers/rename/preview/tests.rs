//! Tests of a rename's preview (ADR 0138, step 14): the groups of lines, only the lines that
//! differ, a long line's part that differs over rows, what is written out, and the bounds.
//!
//! Must not depend on: the app, the window system or the servers' threads.

use std::ops::Range;

use oso_core::buffer::BufferId;
use oso_core::editor::{BufferEdit, Editor};
use oso_core::rope::Rope;

use super::{Snapshot, applied, build, shown, spans};

/// A buffer's id, which the preview never reads.
fn some_buffer() -> BufferId {
    Editor::default().window().buffer()
}

/// A file's path, its text, and its edits.
type File<'a> = (&'a str, &'a str, Vec<(Range<usize>, &'a str)>);

fn preview(files: &[File<'_>]) -> Result<String, &'static str> {
    let snapshot = Snapshot {
        server: "rust-analyzer".into(),
        old: "value".into(),
        new: "count".into(),
        opened: 1,
        files: (files.iter())
            .map(|(path, text, _)| ((*path).to_owned(), Rope::from(*text)))
            .collect(),
    };
    let parts: Vec<BufferEdit> = (files.iter())
        .map(|(_, _, edits)| BufferEdit {
            buffer: some_buffer(),
            revision: 1,
            edits: (edits.iter())
                .map(|(range, new)| (range.clone(), (*new).to_owned()))
                .collect(),
        })
        .collect();
    build(&snapshot, &parts, &|| true).unwrap()
}

#[test]
fn edits_on_lines_that_touch_are_one_group() {
    assert_eq!(
        spans(&[(1, 1), (2, 3), (4, 4), (9, 9)]),
        vec![(1, 4, vec![0, 1, 2]), (9, 9, vec![3])]
    );
    assert!(spans(&[]).is_empty());
}

#[test]
fn edits_apply_in_their_order() {
    let text = "let value = value + 1;";
    assert_eq!(
        applied(text, &[(4..9, "count"), (12..17, "count")]),
        "let count = count + 1;"
    );
    assert_eq!(
        applied("ab", &[(1..1, "x"), (1..1, "y")]),
        "axyb",
        "at one place, in order"
    );
}

#[test]
fn a_shown_line_writes_out_controls_and_bidi() {
    assert_eq!(shown("a\tb\u{202e}c"), "a\\u{9}b\\u{202e}c");
}

#[test]
fn a_buffers_changed_lines_are_shown_before_and_after() {
    let text = "fn main() {\n    let value = 1;\n    println!(\"{value}\");\n}\n";
    let built = preview(&[(
        "src/main.rs",
        text,
        vec![(20..25, "count"), (46..51, "count")],
    )])
    .unwrap();
    assert!(
        built.starts_with(
            "rust-analyzer renames ‘value’ to ‘count’: 2 edits in 1 file (1 opened for it, unshown)\n"
        ),
        "{built}"
    );
    assert!(built.contains("\nsrc/main.rs (2 edits)\n"), "{built}");
    assert!(built.contains("     2 -     let value = 1;\n"), "{built}");
    assert!(
        built.contains("     3 -     println!(\"{value}\");\n"),
        "{built}"
    );
    assert!(built.contains("     2 +     let count = 1;\n"), "{built}");
    assert!(
        built.contains("     3 +     println!(\"{count}\");\n"),
        "{built}"
    );
}

#[test]
fn a_whole_file_rewritten_shows_only_the_lines_that_differ() {
    let text = "a\nb\nlet value = 1;\nc\nd\n";
    let rewritten = "a\nb\nlet count = 1;\nc\nd\n";
    let built = preview(&[("a.rs", text, vec![(0..text.len(), rewritten)])]).unwrap();
    assert!(built.contains("     3 - let value = 1;\n"), "{built}");
    assert!(built.contains("     3 + let count = 1;\n"), "{built}");
    assert!(
        !built.contains(" - a\n") && !built.contains(" + d\n"),
        "{built}"
    );
}

#[test]
fn a_change_far_along_a_long_line_shows_with_its_columns() {
    let line = format!("{}value{}", "x".repeat(450), "y".repeat(450));
    let built = preview(&[("a.rs", &line, vec![(450..455, "count")])]).unwrap();
    assert!(
        built.contains("     1 - (columns 431 to 475 of 905)\n"),
        "{built}"
    );
    assert!(
        built.contains(&format!(
            "       - {}value{}\n",
            "x".repeat(20),
            "y".repeat(20)
        )),
        "{built}"
    );
    assert!(
        built.contains(&format!(
            "       + {}count{}\n",
            "x".repeat(20),
            "y".repeat(20)
        )),
        "{built}"
    );
}

#[test]
fn a_long_new_text_is_shown_whole_over_rows() {
    let new = format!("bar{}; evil()", " ".repeat(400));
    let built = preview(&[("a.rs", "let value = 1;\n", vec![(4..9, &new)])]).unwrap();
    assert!(built.contains("; evil() = 1;"), "{built}");
}

#[test]
fn a_new_text_that_breaks_lines_or_hides_a_change_is_written_out() {
    let built = preview(&[("a.rs", "let value = 1;\n", vec![(4..9, "count\u{202e}x")])]).unwrap();
    assert!(
        built.contains("     1 + let count\\u{202e}x = 1;\n"),
        "{built}"
    );
    let built = preview(&[("a.rs", "let value = 1;\n", vec![(4..9, "a\nb")])]).unwrap();
    assert!(
        built.contains("     1 + let a\n     2 + b = 1;\n"),
        "{built}"
    );
}

#[test]
fn overlapping_or_too_large_edits_are_refused_not_shown_in_part() {
    let text = "let value = 1;\n";
    assert!(preview(&[("a.rs", text, vec![(4..9, "a"), (6..8, "b")])]).is_err());
    let many: String = (0..60).map(|at| format!("line {at}\n")).collect();
    let rewritten: String = (0..60).map(|at| format!("LINE {at}\n")).collect();
    assert!(preview(&[("a.rs", &many, vec![(0..many.len(), &rewritten)])]).is_err());
    let line = "x".repeat(5000);
    assert!(preview(&[("a.rs", &line, vec![(0..5000, "y")])]).is_err());
}

#[test]
fn a_stale_preview_stops() {
    let snapshot = Snapshot {
        server: "s".into(),
        old: "x".into(),
        new: "y".into(),
        opened: 0,
        files: vec![("a.rs".into(), Rope::from("x\n"))],
    };
    let parts = vec![BufferEdit {
        buffer: some_buffer(),
        revision: 1,
        edits: vec![(0..1, "y".into())],
    }];
    assert!(build(&snapshot, &parts, &|| false).is_none());
}
