//! Tests of `M-g d`'s rows (ADR 0138, step 6b): the current buffer's first, the rest by path and
//! line, hints and files outside the root left out, another server's report of a file a buffer
//! holds shown beside the buffer's set, the row selected, the bounds, and a stale build.
//!
//! Must not depend on: the app, threads or the file system.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::{Baseline, Buffer, BufferId};
use oso_core::diagnostic::{Diagnostics, New, Severity, Tags};
use oso_core::editor::{DiagnosticRows, Editor};
use oso_core::rope::Rope;
use oso_lsp::position::{Encoding, Position};
use oso_lsp::protocol::{Published, Reported};

use super::{OpenSet, ROW_PATH_BYTES_MAX, ROWS_BYTES_MAX, Snapshot, build};

/// An editor showing a buffer of `/w/oso/z.rs`, whose list of diagnostics is open, the first,
/// and another buffer's id.
fn listing() -> (Editor, BufferId, BufferId) {
    let path = PathBuf::from("/w/oso/z.rs");
    let mut shown = Buffer::visiting(path.clone());
    shown.set_baseline(Baseline {
        location: path,
        file: None,
    });
    let mut editor = Editor::with_buffer(shown);
    let current = editor.window().buffer();
    let other = editor.add_buffer(Buffer::new("b.rs"));
    editor.open_diagnostics_list("Diagnostics").unwrap();
    assert_eq!(editor.take_diagnostics_request().unwrap().generation, 1);
    (editor, current, other)
}

fn open(buffer: BufferId, path: &str, text: &str, at: &[(usize, Severity)]) -> OpenSet {
    let entries = at.iter().map(|&(start, severity)| New {
        range: start..start + 1,
        severity,
        tags: Tags::default(),
        source: "rustc",
        code: "E0308",
        message: "in a  buffer",
    });
    OpenSet {
        buffer,
        path: PathBuf::from(path),
        text: Rope::from(text),
        revision: 3,
        set: Diagnostics::new("rust-analyzer", entries, text.len()),
    }
}

fn report(at: &[(u32, Severity)]) -> Arc<Published> {
    let diagnostics = at
        .iter()
        .map(|&(line, severity)| Reported {
            start: Position { line, character: 2 },
            end: Position { line, character: 3 },
            severity,
            tags: Tags::default(),
            source: "clippy".into(),
            code: String::new(),
            message: format!("on line {line}\nmore"),
        })
        .collect();
    Arc::new(Published {
        uri: String::new(),
        version: None,
        diagnostics,
    })
}

fn snapshot(
    current: BufferId,
    open: Vec<OpenSet>,
    files: Vec<(PathBuf, Arc<Published>, Encoding)>,
) -> Snapshot {
    Snapshot {
        generation: 1,
        root: PathBuf::from("/w/oso"),
        current,
        cursor: 5,
        open,
        files: files
            .into_iter()
            .map(|(path, report, encoding)| (Arc::<Path>::from(path), report, encoding))
            .collect(),
    }
}

/// The rows of `snapshot`, with no newer list to stop them.
fn built(snapshot: &Snapshot) -> DiagnosticRows {
    build(snapshot, &|| true).expect("rows, as nothing stopped them")
}

/// The rows' texts, the row selected and whether the list was cut, as `editor` shows them.
fn taken(editor: &mut Editor, rows: DiagnosticRows) -> (Vec<String>, Option<usize>, bool) {
    editor.diagnostics_listed(rows);
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    let shown = (0..candidates.len())
        .map(|place| candidates.get(place).unwrap().text().to_owned())
        .collect();
    (shown, candidates.selected(), editor.files_cut())
}

#[test]
fn the_current_buffer_s_come_first_then_each_file_s_by_path_and_line() {
    let (mut editor, current, other) = listing();
    let snapshot = snapshot(
        current,
        vec![
            open(other, "/w/oso/b.rs", "x\n", &[(0, Severity::Error)]),
            open(
                current,
                "/w/oso/z.rs",
                "one\ntwo\nthree\n",
                &[
                    (1, Severity::Warning),
                    (8, Severity::Error),
                    (9, Severity::Hint),
                ],
            ),
        ],
        vec![
            (
                PathBuf::from("/w/oso/src/c.rs"),
                report(&[
                    (9, Severity::Error),
                    (2, Severity::Hint),
                    (4, Severity::Warning),
                ]),
                Encoding::Utf16,
            ),
            (
                PathBuf::from("/w/oso/a.rs"),
                report(&[(0, Severity::Information)]),
                Encoding::Utf8,
            ),
            // Another server's report of a file a buffer holds shows beside the buffer's set; one
            // outside the root not at all.
            (
                PathBuf::from("/w/oso/b.rs"),
                report(&[(5, Severity::Error)]),
                Encoding::Utf8,
            ),
            (
                PathBuf::from("/w/other/d.rs"),
                report(&[(0, Severity::Error)]),
                Encoding::Utf8,
            ),
        ],
    );
    let (shown, selected, cut) = taken(&mut editor, built(&snapshot));
    assert_eq!(
        shown,
        [
            "z.rs:1: warning: in a buffer",
            "z.rs:3: error: in a buffer",
            "a.rs:1: info: on line 0",
            "b.rs:1: error: in a buffer",
            "b.rs:6: error: on line 5",
            "src/c.rs:5: warning: on line 4",
            "src/c.rs:10: error: on line 9",
        ]
    );
    assert_eq!(selected, Some(1), "the first at or after the cursor, at 5");
    assert!(!cut);
}

#[test]
fn with_none_after_the_cursor_the_first_row_is_selected_and_with_none_no_row_is() {
    let (mut editor, current, _) = listing();
    let snapshot = snapshot(
        current,
        vec![open(
            current,
            "/w/oso/z.rs",
            "ab\n",
            &[(1, Severity::Error)],
        )],
        vec![(
            PathBuf::from("/w/oso/a.rs"),
            report(&[(0, Severity::Error)]),
            Encoding::Utf8,
        )],
    );
    assert_eq!(taken(&mut editor, built(&snapshot)).1, Some(0));
    let (mut editor, current, _) = listing();
    let (shown, selected, _) = taken(&mut editor, built(&self::snapshot(current, vec![], vec![])));
    assert!(shown.is_empty());
    assert_eq!(selected, None);
}

#[test]
fn past_the_bound_no_more_rows_are_made_and_the_list_says_so() {
    use oso_lsp::limits::{DIAGNOSTIC_ROWS_MAX, DIAGNOSTICS_MAX};
    let (mut editor, current, _) = listing();
    let lines: Vec<(u32, Severity)> = (0..DIAGNOSTICS_MAX as u32)
        .map(|line| (line, Severity::Error))
        .collect();
    let files = (0..DIAGNOSTIC_ROWS_MAX / DIAGNOSTICS_MAX + 1)
        .map(|n| {
            (
                PathBuf::from(format!("/w/oso/f{n:03}.rs")),
                report(&lines),
                Encoding::Utf16,
            )
        })
        .collect();
    let rows = built(&snapshot(current, Vec::new(), files));
    let (shown, _, cut) = taken(&mut editor, rows);
    assert_eq!(shown.len(), DIAGNOSTIC_ROWS_MAX);
    assert!(cut);
    assert!(shown[0].starts_with("f000.rs:1:"));
}

#[test]
fn a_long_path_gives_no_row_and_the_rows_bytes_are_bounded() {
    let (mut editor, current, _) = listing();
    let long = format!("/w/oso/{}.rs", "x".repeat(ROW_PATH_BYTES_MAX));
    let files = vec![(
        PathBuf::from(long),
        report(&[(0, Severity::Error)]),
        Encoding::Utf8,
    )];
    let (shown, _, _) = taken(&mut editor, built(&snapshot(current, Vec::new(), files)));
    assert!(shown.is_empty(), "each row would hold the whole path");
    // Messages of 1 KiB each, past the bytes the rows may hold well before the rows' bound.
    let mut wordy = report(&[(0, Severity::Error)]).as_ref().clone();
    wordy.diagnostics[0].message = "m".repeat(2048);
    let one = wordy.diagnostics[0].clone();
    wordy.diagnostics = vec![one; 2000];
    let wordy = Arc::new(wordy);
    let files = (0..(ROWS_BYTES_MAX / (2000 * 1024) + 2))
        .map(|n| {
            (
                PathBuf::from(format!("/w/oso/f{n:03}.rs")),
                Arc::clone(&wordy),
                Encoding::Utf8,
            )
        })
        .collect();
    let (mut editor, current, _) = listing();
    let (shown, _, cut) = taken(&mut editor, built(&snapshot(current, Vec::new(), files)));
    assert!(cut);
    assert!(shown.len() <= ROWS_BYTES_MAX / 1024, "{}", shown.len());
}

#[test]
fn a_newer_list_stops_the_build() {
    let (_, current, _) = listing();
    let files = vec![(
        PathBuf::from("/w/oso/a.rs"),
        report(&[(0, Severity::Error)]),
        Encoding::Utf8,
    )];
    assert!(build(&snapshot(current, Vec::new(), files), &|| false).is_none());
}

/// On Windows, the servers' paths are spelled as a URI reads back, the drive letter upper case,
/// whatever the root's spelling.
#[cfg(windows)]
#[test]
fn a_root_spelled_otherwise_than_the_servers_paths_still_holds_them() {
    let (mut editor, current, _) = listing();
    let mut snapshot = snapshot(
        current,
        Vec::new(),
        vec![(
            PathBuf::from(r"C:\w\oso\a.rs"),
            report(&[(0, Severity::Error)]),
            Encoding::Utf8,
        )],
    );
    snapshot.root = PathBuf::from(r"c:\w\oso");
    let (shown, _, _) = taken(&mut editor, built(&snapshot));
    assert_eq!(shown, ["a.rs:1: error: on line 0"]);
}
