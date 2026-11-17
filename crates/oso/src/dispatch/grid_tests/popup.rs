//! The completion popup at point in the text grid, and its snapshots (ADR 0138, step 9): under
//! the word's start, its rows with their annotations, the selection moving and wrapping, typing
//! narrowing it, and over the word's row where too few rows fit below.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::{Buffer, ServerMark};
use oso_core::editor::{CompletionPopupAnswer, CompletionPopupItem, Editor};
use oso_core::frame::FrameSize;
use oso_core::selection::SelectionSet;

use oso_core::minibuffer;
use oso_core::window::Screen;

use super::{aids_off, check_editor, spans_row};

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
        .map(|label| CompletionPopupItem {
            label: (*label).into(),
            annotation: "function fn()".into(),
            filter: None,
            text: (*label).into(),
            range: None,
            extra: Vec::new(),
        })
        .collect();
    let at = editor.ask_at();
    let answer = CompletionPopupAnswer {
        serial: 1,
        incomplete: false,
        at,
        revision: at.revision,
        items,
    };
    editor.open_completion_popup(answer).unwrap();
    editor
}

/// The completion popup, if one is open: where it is, then its rows, the selected one with `>`
/// beside it and what matched marked `*` below (ADR 0138, step 9).
pub(super) fn draw_popup(editor: &Editor, lines: &mut Vec<String>) {
    let Some(popup) = editor.completion_popup() else {
        return;
    };
    let mut screen = Screen::default();
    editor.screen(&mut screen);
    let Some(regions) = editor.completion_popup_regions(&screen) else {
        lines.push("popup with no room".to_owned());
        return;
    };
    let (outer, inner) = (regions.outer, regions.inner);
    lines.push(format!(
        "popup at row {}, column {}: {} x {}",
        outer.row, outer.column, outer.rows, outer.columns
    ));
    let named = popup.rows().map(|(_, row)| (row.text(), row.binding()));
    let column = minibuffer::annotation_column(named, inner.columns);
    let mut highlights = Vec::new();
    for (selected, row) in popup.rows().take(inner.rows) {
        let (text, note) = (row.text(), row.annotation());
        let at = minibuffer::candidate_row(text, "", note, inner.columns, column);
        let shown = &text[..at.text];
        let mut spans = vec![shown.to_owned()];
        let used = minibuffer::cells_of(shown);
        if let Some((column, bytes, _)) = at.annotation {
            spans.push(" ".repeat(column - used) + &note[..bytes]);
        }
        let spans: Vec<&str> = spans.iter().map(String::as_str).collect();
        let (line, _) = spans_row(&spans, inner.columns);
        let side = if selected { '>' } else { ' ' };
        lines.push(format!("│{side}{line} │"));
        popup.highlights(row, &mut highlights);
        let mut marks = vec![' '; inner.columns];
        for range in &highlights {
            let end = range.end.min(shown.len());
            let (from, to) = (
                minibuffer::cells_of(&shown[..range.start.min(end)]),
                minibuffer::cells_of(&shown[..end]).min(inner.columns),
            );
            marks[from..to].fill('*');
        }
        if marks.contains(&'*') {
            lines.push(format!(": {} :", marks.into_iter().collect::<String>()));
        }
    }
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
