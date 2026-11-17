//! Tests of a rename's preview (ADR 0138, step 14): the groups of lines, the edits made in
//! them, a buffer's lines and a file's read in its encoding, what is written out, and a
//! file that cannot be read.
//!
//! Must not depend on: the app, the window system or the servers' threads.

use std::path::PathBuf;

use oso_core::rope::Rope;
use oso_lsp::position::{Encoding, Position};

use super::{FileSnap, Snapshot, Source, applied, build, shown, spans};

fn snapshot(files: Vec<FileSnap>, root: PathBuf) -> Snapshot {
    Snapshot {
        server: "rust-analyzer".into(),
        old: "value".into(),
        new: "count".into(),
        root,
        home: None,
        files,
    }
}

#[test]
fn edits_on_lines_that_touch_are_one_group() {
    assert_eq!(
        spans(&[(4, 4), (1, 1), (2, 3), (9, 9)]),
        vec![(1, 4, vec![1, 2, 0]), (9, 9, vec![3])]
    );
    assert!(spans(&[]).is_empty());
}

#[test]
fn edits_apply_in_their_order_and_an_overlapping_one_is_left_out() {
    let text = "let value = value + 1;";
    assert_eq!(
        applied(text, &[(12..17, "count"), (4..9, "count")]),
        "let count = count + 1;"
    );
    assert_eq!(
        applied(text, &[(4..9, "a"), (6..8, "b")]),
        "let a = value + 1;"
    );
}

#[test]
fn a_shown_line_writes_out_controls_and_bidi_and_is_cut() {
    assert_eq!(shown("a\tb\u{202e}c"), "a\\u{9}b\\u{202e}c");
    let long = "x".repeat(500);
    let cut = shown(&long);
    assert!(cut.ends_with('…'));
    assert_eq!(cut.chars().count(), super::SHOWN_MAX + 1);
}

#[test]
fn a_buffers_lines_are_shown_before_and_after() {
    let text = Rope::from("fn main() {\n    let value = 1;\n    println!(\"{value}\");\n}\n");
    let files = vec![FileSnap {
        shown: "src/main.rs".into(),
        source: Source::Text {
            text,
            edits: vec![(20..25, "count".into()), (46..51, "count".into())],
        },
    }];
    let built = build(&snapshot(files, PathBuf::from("/p")), &|| true).unwrap();
    assert!(
        built.starts_with("rust-analyzer renames ‘value’ to ‘count’: 2 edits in 1 file\n"),
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
fn a_new_text_that_breaks_lines_or_hides_a_change_is_written_out() {
    let text = Rope::from("let value = 1;\n");
    let files = vec![FileSnap {
        shown: "a.rs".into(),
        source: Source::Text {
            text,
            edits: vec![(4..9, "count\u{202e}x".into())],
        },
    }];
    let built = build(&snapshot(files, PathBuf::from("/p")), &|| true).unwrap();
    assert!(
        built.contains("     1 + let count\\u{202e}x = 1;\n"),
        "{built}"
    );
}

#[test]
fn a_files_lines_are_read_from_the_root_and_placed_in_its_encoding() {
    let dir = std::env::temp_dir().join(format!("oso-rename-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "// 名前\npub fn value() {}\n").unwrap();
    let root = std::fs::canonicalize(&dir).unwrap();
    let files = vec![FileSnap {
        shown: "src/lib.rs".into(),
        source: Source::File {
            path: root.join("src/lib.rs"),
            edits: vec![(Position::new(1, 7), Position::new(1, 12), "count".into())],
            encoding: Encoding::Utf16,
        },
    }];
    let built = build(&snapshot(files, root.clone()), &|| true).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(built.contains("(1 not open)"), "{built}");
    assert!(built.contains("     2 - pub fn value() {}\n"), "{built}");
    assert!(built.contains("     2 + pub fn count() {}\n"), "{built}");
}

#[test]
fn a_file_that_cannot_be_read_shows_the_new_text() {
    let root = PathBuf::from("/nonexistent-oso-root");
    let files = vec![FileSnap {
        shown: "gone.rs".into(),
        source: Source::File {
            path: root.join("gone.rs"),
            edits: vec![(Position::new(3, 0), Position::new(3, 5), "count".into())],
            encoding: Encoding::Utf8,
        },
    }];
    let built = build(&snapshot(files, root), &|| true).unwrap();
    assert!(
        built.contains("     4   (the line could not be read whole"),
        "{built}"
    );
    assert!(built.contains("     4 + count\n"), "{built}");
}

#[test]
fn a_stale_preview_stops() {
    let files = vec![FileSnap {
        shown: "a.rs".into(),
        source: Source::Text {
            text: Rope::from("x\n"),
            edits: vec![(0..1, "y".into())],
        },
    }];
    assert!(build(&snapshot(files, PathBuf::from("/p")), &|| false).is_none());
}
