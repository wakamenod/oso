//! Tests that the commands that make a buffer secret drop its highlights and their log: a yank
//! of a secret kill, and a recovery from a copy that may hold secrets (ADR 0111).
//!
//! Must not depend on: `command`; these drive the editor directly.

use std::path::PathBuf;

use super::Editor;
use crate::buffer::{Buffer, BufferId, Mode, Verdict};
use crate::display_map::TabWidth;
use crate::load::decode_all;
use crate::syntax::{Highlight, Highlights};
use crate::window::ViewSize;

/// An editor showing a Rust buffer of `text` that holds highlights, with the revision they are
/// for.
fn highlighted(text: &str) -> (Editor, BufferId, u64) {
    let mut buffer = Buffer::from_text("main.rs", text);
    buffer.set_mode(Mode::Rust);
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(10, 40));
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    let revision = buffer.revision();
    let mut highlights = Highlights::new(text.len());
    highlights.replace(0..2, [(2, Some(Highlight::Keyword))]);
    assert!(buffer.take_highlights(revision, highlights));
    (editor, id, revision)
}

#[test]
fn yanking_a_secret_kill_drops_the_highlights_and_their_log() {
    let (mut editor, id, revision) = highlighted("fn main() {}\n");
    let piece = crate::kill::Piece::from(crate::rope::Rope::from("KEY"));
    editor.kills.ring.push(vec![piece], true);
    editor.begin_undo_group();
    editor.yank(0, false).unwrap();
    editor.end_undo_group();
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.verdict(), Verdict::Yanked);
    assert!(
        !buffer.keeps_highlighting(),
        "nothing kept, not only hidden"
    );
    assert!(buffer.highlights().is_none());
    assert_eq!(buffer.splices_since(revision), None);
    editor.undo().unwrap();
    let buffer = editor.buffer(id).unwrap();
    assert!(
        !buffer.keeps_highlighting(),
        "undo leaves it secret, and logs nothing"
    );
}

#[test]
fn a_recovery_from_a_secret_copy_drops_the_highlights_and_their_log() {
    let (mut editor, id, revision) = highlighted("fn main() {}\n");
    let view = editor.add_internal_buffer("*copy of main.rs*", Mode::CopyView);
    let snapshot = decode_all(b"fn copy() {}\n", TabWidth::default());
    let copy = editor.buffer_mut(view).unwrap();
    copy.apply_load(snapshot);
    copy.judge_copy(&[PathBuf::from("/p/.env")], Verdict::SecretName);
    editor.begin_undo_group();
    editor.replace_text_from(id, view).unwrap();
    editor.end_undo_group();
    let buffer = editor.buffer(id).unwrap();
    assert!(buffer.is_secret());
    assert!(
        !buffer.keeps_highlighting(),
        "nothing kept, not only hidden"
    );
    assert_eq!(buffer.splices_since(revision), None);
}

#[test]
fn shown_ranges_are_what_the_window_shows() {
    let line = "fn f() {}\n";
    let (mut editor, id, _) = highlighted(&line.repeat(100));
    let mut shown = Vec::new();
    editor.shown_ranges(&mut shown);
    assert_eq!(shown.len(), 1);
    let (buffer, range) = shown[0].clone();
    assert_eq!(buffer, id);
    assert_eq!(range.start, 0);
    // 10 cells high, a modeline among them: a few lines, not the hundred.
    assert!(
        range.end > line.len() && range.end < 20 * line.len(),
        "{range:?}"
    );
    editor.scroll_view(editor.tiled().0.id(), 30);
    editor.shown_ranges(&mut shown);
    assert_eq!(shown[0].1.start, 30 * line.len(), "{:?}", shown[0].1);
}

#[test]
fn a_visited_file_takes_its_mode_from_its_name() {
    assert_eq!(
        Buffer::visiting(PathBuf::from("/p/main.rs")).mode(),
        Mode::Rust
    );
    assert_eq!(
        Buffer::visiting(PathBuf::from("/p/README.md")).mode(),
        Mode::Markdown
    );
    assert_eq!(
        Buffer::visiting(PathBuf::from("/p/notes.txt")).mode(),
        Mode::Fundamental
    );
}

#[test]
fn a_new_name_gives_the_mode_as_a_visit_does() {
    let mut buffer = Buffer::visiting(PathBuf::from("/p/notes.txt"));
    buffer.set_path(PathBuf::from("/p/notes.md"));
    assert_eq!(buffer.mode(), Mode::Markdown);
}

#[test]
fn a_file_buffer_never_takes_the_copies_modes_and_an_internal_one_keeps_its_own() {
    let mut file = Buffer::visiting(PathBuf::from("/p/main.rs"));
    assert!(!file.set_mode(Mode::CopyList));
    assert!(!file.set_mode(Mode::CopyView));
    assert_eq!(file.mode(), Mode::Rust);
    assert!(file.set_mode(Mode::Toml));
    let mut editor = Editor::with_buffer(Buffer::from_text("a", ""));
    let list = editor.add_internal_buffer("*Copies*", Mode::CopyList);
    let list = editor.buffer_mut(list).unwrap();
    assert!(!list.set_mode(Mode::Fundamental));
    assert_eq!(list.mode(), Mode::CopyList);
}
