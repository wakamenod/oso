//! Text-grid snapshots of jumps (ADR 0116): a string's matches while it is read, labels in two
//! windows, labels cut at the row's end and at the next label, and line starts and ends.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;
use oso_core::frame::FrameSize;
use oso_core::jump::Keys;

use super::{aids_off, check, check_editor, check_frame};

/// An editor on `text` in a frame of `rows` by `columns`, whose labels take `keys`.
fn with_keys(text: &str, (rows, columns): (usize, usize), keys: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("test", text));
    aids_off(&mut editor);
    editor.set_frame_size(FrameSize { rows, columns });
    editor.set_jump_keys(Keys::new(keys).unwrap());
    editor
}

#[test]
fn a_jump_highlights_its_strings_matches_then_labels_them_nearest_first() {
    check(
        "jump_reading",
        "xab ab\nab abc\n",
        (2, 12),
        &["M-j a", "b", "DEL", "b RET", "s", "M-j a b RET C-g"],
    );
}

#[test]
fn labels_show_in_every_window_and_one_in_the_other_selects_it() {
    let text = "one two\ntwo one\n";
    check_frame(
        "jump_two_windows",
        text,
        (8, 41),
        &["C-x 3", "M-j o RET", "h"],
    );
}

#[test]
fn a_label_is_cut_at_the_rows_end_and_at_the_next_label() {
    // Three keys and seven targets: labels of one and two keys.
    let editor = with_keys("aaa a\nxxxxxxxxxa\naa\n", (5, 10), "qwe");
    check_editor(
        "jump_labels_cut",
        b"aaa a\nxxxxxxxxxa\naa\n",
        editor,
        &["M-j a RET", "w"],
    );
}

#[test]
fn line_starts_and_ends_are_labelled_an_empty_lines_after_its_text() {
    check(
        "jump_lines",
        "one\n\n  three\n",
        (4, 10),
        &["M-g f", "C-g", "run:avy-goto-end-of-line"],
    );
}
