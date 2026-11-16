//! Text-grid snapshots of the completion popup at point (ADR 0138, step 9): under the word's
//! start, its rows with their annotations, the selection moving and wrapping, typing narrowing
//! it, and over the word's row where too few rows fit below.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::{Buffer, ServerMark};
use oso_core::editor::{Editor, PopupAnswer, PopupItem};
use oso_core::frame::FrameSize;
use oso_core::selection::SelectionSet;

use super::{aids_off, check_editor};

/// An editor on `text` in a frame of `rows` by `columns`, which a server follows, its cursor at
/// `at`, with the popup open over `labels`, each a function.
fn popped(text: &str, (rows, columns): (usize, usize), at: usize, labels: &[&str]) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", text));
    aids_off(&mut editor);
    editor.set_frame_size(FrameSize { rows, columns });
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.set_followed(true);
    buffer.set_server(Some(ServerMark::Ready));
    editor.fit_gutter();
    editor.current_mut().0.selections = SelectionSet::new(at);
    let items = labels
        .iter()
        .map(|label| PopupItem {
            label: (*label).into(),
            annotation: "function fn()".into(),
            filter: None,
            text: (*label).into(),
            range: None,
        })
        .collect();
    let at = editor.ask_at();
    let answer = PopupAnswer {
        at,
        revision: at.revision,
        items,
    };
    editor.open_popup(answer).unwrap();
    editor
}

#[test]
fn the_popup_shows_under_the_word_moves_with_wrap_narrows_and_inserts() {
    let text = "fn main() {\n    let x = fo\n}\n";
    let editor = popped(text, (12, 50), 26, &["foo", "format", "for_each"]);
    let steps = ["", "C-p", "C-n", "r", "TAB"];
    check_editor("popup_below", text.as_bytes(), editor, &steps);
}

#[test]
fn the_popup_goes_over_the_word_where_too_few_rows_fit_below() {
    let text = "a\nb\nc\nd\nfo\n";
    let editor = popped(text, (8, 40), 10, &["foo", "format", "for_each", "fold"]);
    check_editor("popup_above", text.as_bytes(), editor, &["", "C-g"]);
}
