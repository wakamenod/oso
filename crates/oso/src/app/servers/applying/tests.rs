//! Tests of what a server's edit of files the server has no text of refuses (ADR 0138, step
//! 13b): before anything opens, once a file is judged and read, and the question it asks.
//!
//! Must not depend on: the window system, threads or the file system.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use oso_core::buffer::{Baseline, Buffer, FileStamp};
use oso_core::display_map::TabWidth;
use oso_core::editor::Editor;
use oso_core::load::decode_all;

use super::{At, judge_loaded, judge_unsent, question_of};

const ROOT: &str = "/w/proj";

/// A buffer visiting `path`, loaded with `text`, judged ordinary, its file found at `location`
/// if `there`.
fn visited(path: &str, location: &str, there: bool) -> Buffer {
    let path = PathBuf::from(path);
    let mut buffer = Buffer::visiting(path.clone());
    buffer.judge_secret(Some(std::slice::from_ref(&path)));
    buffer.apply_load(decode_all(b"fn a() {}\n", TabWidth::default()));
    buffer.set_baseline(Baseline {
        location: PathBuf::from(location),
        file: there.then_some(FileStamp {
            modified: SystemTime::UNIX_EPOCH,
            len: 10,
            mode: 0o644,
            identity: None,
        }),
    });
    buffer
}

#[test]
fn before_anything_opens_a_file_outside_in_git_or_secret_or_a_held_buffer_changed_refuses() {
    let mut editor = Editor::default();
    let root = Path::new(ROOT);
    let judged =
        |editor: &Editor, path: &str, held| judge_unsent(editor, root, (Path::new(path), held));
    assert_eq!(
        judged(&editor, "/w/other/a.rs", None),
        Err("it edits a file outside the project")
    );
    assert_eq!(
        judged(&editor, "/w/proj/.git/config", None),
        Err("it edits a file in a .git directory")
    );
    assert_eq!(
        judged(&editor, "/w/proj/.env", None),
        Err("it edits a file that may hold secrets")
    );
    assert_eq!(judged(&editor, "/w/proj/src/lib.rs", None), Ok(At::ToOpen));
    let held = editor.add_buffer(visited("/w/proj/src/lib.rs", "/w/proj/src/lib.rs", true));
    let revision = editor.buffer(held).unwrap().revision();
    assert_eq!(
        judged(&editor, "/w/proj/src/lib.rs", Some(held)),
        Ok(At::Held(held, revision))
    );
    editor
        .apply_edits(
            held,
            &[oso_core::buffer::Replacement {
                range: 0..0,
                text: "x",
            }],
        )
        .unwrap();
    assert_eq!(
        judged(&editor, "/w/proj/src/lib.rs", Some(held)),
        Err("a buffer it edits has changes the server has not seen")
    );
    // One still loading is waited for.
    let loading = editor.add_buffer(Buffer::visiting(PathBuf::from("/w/proj/src/b.rs")));
    assert_eq!(
        judged(&editor, "/w/proj/src/b.rs", Some(loading)),
        Ok(At::Loading(loading))
    );
}

#[test]
fn once_loaded_a_file_not_there_led_outside_or_a_held_buffer_changed_refuses() {
    let mut editor = Editor::default();
    let roots = (Path::new(ROOT), Path::new(ROOT));
    let path = Path::new("/w/proj/src/lib.rs");
    let fine = editor.add_buffer(visited("/w/proj/src/lib.rs", "/w/proj/src/lib.rs", true));
    let revision = editor.buffer(fine).unwrap().revision();
    assert_eq!(
        judge_loaded(&editor, (fine, None), path, roots),
        Ok(revision)
    );
    assert_eq!(
        judge_loaded(&editor, (fine, Some(revision)), path, roots),
        Ok(revision)
    );
    assert_eq!(
        judge_loaded(&editor, (fine, Some(revision + 1)), path, roots),
        Err("a buffer it edits changed while it waited")
    );
    // Through a link in the project, to a file out of it.
    let out = editor.add_buffer(visited("/w/proj/vendor/x.rs", "/home/me/x.rs", true));
    assert_eq!(
        judge_loaded(
            &editor,
            (out, None),
            Path::new("/w/proj/vendor/x.rs"),
            roots
        ),
        Err("a file it edits leads outside the project")
    );
    let missing = editor.add_buffer(visited("/w/proj/src/gone.rs", "/w/proj/src/gone.rs", false));
    assert!(
        judge_loaded(
            &editor,
            (missing, None),
            Path::new("/w/proj/src/gone.rs"),
            roots
        )
        .is_err()
    );
    let gone = editor.add_buffer(visited("/w/proj/src/c.rs", "/w/proj/src/c.rs", true));
    editor.kill_buffer(gone);
    assert!(judge_loaded(&editor, (gone, None), path, roots).is_err());
}

#[test]
fn the_question_names_the_counts_the_first_files_and_the_title() {
    let root = Path::new(ROOT);
    let one = [Path::new("/w/proj/src/lib.rs")];
    assert_eq!(
        question_of("ra", "Fix", (1, &one), root),
        "ra: apply to 1 file, 1 not open (src/lib.rs)? \u{201c}Fix\u{201d}"
    );
    let paths: Vec<PathBuf> = (0..5)
        .map(|n| PathBuf::from(format!("/w/proj/{n}\u{202e}.rs")))
        .collect();
    let many: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let asked = question_of("ra", "Fix", (6, &many), root);
    assert!(
        asked.contains("5 not open (0\\u{202e}.rs, 1\\u{202e}.rs, 2\\u{202e}.rs and 2 more)"),
        "{asked}"
    );
}
