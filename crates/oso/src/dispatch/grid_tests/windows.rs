//! Text-grid snapshots of split windows (ADR 0112): stacked and side by side windows on one
//! buffer, selecting, resizing, balancing and deleting them, an edit seen in both, a wrap at
//! two widths, a killed buffer, and a buffer that may hold secrets in the window not selected.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use std::path::PathBuf;

use oso_core::buffer::Buffer;
use oso_core::editor::{Editor, VisualAids};
use oso_core::frame::FrameSize;

use super::{aids_off, check_editor, check_frame};

fn lines(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn split_below_then_move_select_and_delete() {
    let steps = [
        "",
        "run:split-window-below",
        "C-n C-n C-f",
        "run:other-window",
        "run:enlarge-window 2",
        "run:delete-window",
    ];
    check_frame("windows_below", &lines(20), (12, 24), &steps);
}

#[test]
fn split_right_puts_a_divider_between_them() {
    let steps = [
        "run:split-window-right",
        "run:split-window-right-and-select",
        "run:shrink-window-horizontally 3",
        "run:balance-windows",
        "run:windmove-left",
        "run:delete-other-windows",
    ];
    check_frame("windows_right", &lines(6), (8, 47), &steps);
}

#[test]
fn only_the_selected_window_has_the_band_and_the_region() {
    let text = lines(6);
    let mut editor = Editor::with_buffer(Buffer::from_text("test", &text));
    aids_off(&mut editor);
    editor.set_visual_aids(VisualAids {
        current_line: true,
        line_numbers: true,
        ..editor.visual_aids()
    });
    editor.set_frame_size(FrameSize {
        rows: 10,
        columns: 47,
    });
    let steps = [
        "run:split-window-right",
        "C-SPC C-n C-f",
        "run:other-window",
        "C-s l i n e",
    ];
    check_editor("windows_selected_aids", text.as_bytes(), editor, &steps);
}

#[test]
fn an_edit_in_one_window_shows_in_the_other_on_the_same_buffer() {
    let steps = [
        "run:split-window-below",
        "run:other-window",
        "M->",
        "run:other-window",
        "x y z RET",
        "C-a C-k C-k",
    ];
    check_frame("windows_one_buffer", &lines(5), (12, 24), &steps);
}

#[test]
fn each_window_wraps_at_its_own_width() {
    let text = "The quick brown fox jumps over the lazy dog and keeps on running.\nnext\n";
    let steps = [
        "C-x x t",
        "run:split-window-right",
        "run:enlarge-window-horizontally 6",
    ];
    check_frame("windows_wrap", text, (8, 47), &steps);
}

#[test]
fn killing_a_buffer_shown_in_both_windows_moves_both_back() {
    let text = lines(3);
    let mut editor = Editor::with_buffer(Buffer::from_text("test", &text));
    aids_off(&mut editor);
    editor.set_frame_size(FrameSize {
        rows: 10,
        columns: 30,
    });
    let notes = editor.add_buffer(Buffer::from_text("notes", "a note\n"));
    editor.show(notes);
    let steps = ["run:split-window-below", "C-x k RET"];
    check_editor("windows_kill_buffer", text.as_bytes(), editor, &steps);
}

#[test]
fn a_buffer_that_may_hold_secrets_in_the_window_not_selected_shows_no_position() {
    let text = lines(3);
    let mut editor = Editor::with_buffer(Buffer::from_text("test", &text));
    aids_off(&mut editor);
    editor.set_frame_size(FrameSize {
        rows: 10,
        columns: 40,
    });
    let path = PathBuf::from("/tmp/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.finish_empty();
    secret.judge_secret(Some(&[path]));
    let secret = editor.add_buffer(secret);
    editor.show(secret);
    let steps = ["run:split-window-below-and-select", "C-x b t e s t RET"];
    check_editor("windows_secret_beside", text.as_bytes(), editor, &steps);
}
