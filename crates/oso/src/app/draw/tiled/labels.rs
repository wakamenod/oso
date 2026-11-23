//! A jump's labels in a tiled window, over its text, as a frame lists them (ADR 0116).
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.

use oso_core::frame::CellRect;
use oso_render::{Face, Span};

use super::WindowCells;

/// A jump's labels in a window over its text: each label's first key in [`Face::JumpLabel`],
/// and the keys after it in [`Face::JumpLabelNext`], in the order the editor gives them, by
/// row and then column (ADR 0116).
pub(super) fn labels_of<'a>(cells: &'a WindowCells, out: &mut Vec<(CellRect, Span<'a>)>) {
    let jump = &cells.jump;
    for label in &jump.labels {
        let text = &jump.text[label.keys.clone()];
        let (first, rest) = text.split_at(text.len().min(1));
        let one = CellRect {
            columns: 1,
            ..label.cells
        };
        out.push((
            one,
            Span {
                text: first,
                face: Face::JumpLabel,
            },
        ));
        if !rest.is_empty() {
            let after = CellRect {
                column: label.cells.column + 1,
                columns: label.cells.columns - 1,
                ..label.cells
            };
            out.push((
                after,
                Span {
                    text: rest,
                    face: Face::JumpLabelNext,
                },
            ));
        }
    }
}

/// How many entries [`labels_of`] gives for a window.
pub(super) fn label_runs(cells: &WindowCells) -> usize {
    let labels = &cells.jump.labels;
    labels.len()
        + labels
            .iter()
            .filter(|label| label.cells.columns > 1)
            .count()
}
