//! A native session's footer (ADR 0165, the look, part G2; ADR 0170): the row under what is docked
//! in its window, the permission mode and its key at the left and the model at the right. It is
//! drawn, and is no text of a buffer's, so nothing that reads a buffer or goes to a window meets it.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.
//!
//! - The mode's words are in a face of the mode's kind, dim for a quiet one; all else is dim. The
//!   model is in a pane of its own.

use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::window::{DockSlot, WindowId};
use oso_render::{Bands, Face, Layer, Pane, PaneContent, Rules, Span};

use super::composer::is_composer;
use super::{Part, pane_id};
use crate::app::draw::reuse;
use crate::app::sessions::NativeSessions;
use crate::sessions::native::footer::Tone;

/// What the footer starts with: its words are in line with the composer's text.
const INDENT: &str = "  ";
/// The blank column after the model.
const MARGIN: usize = 1;

/// A window's footer: where it is, the spans at its left, and the model's at its right and the
/// columns it has there.
pub(in crate::app::draw) type Row<'a> = (WindowId, CellRect, [Span<'a>; 4], ([Span<'a>; 1], usize));

/// The lines of a footer: its left's and its right's.
pub(in crate::app::draw) type Lines<'a> = [[&'a [Span<'a>]; 1]; 2];

/// Room for the footers of a frame and their lines, empty between frames.
#[derive(Default)]
pub(in crate::app::draw) struct Room {
    rows: Vec<Row<'static>>,
    lines: Vec<Lines<'static>>,
}

/// The face of a mode of kind `tone`, on the text's background.
fn face(tone: Tone) -> Face {
    match tone {
        Tone::Quiet => Face::Prompt,
        Tone::Edits => Face::SessionCode,
        Tone::Plan => Face::DiagnosticInfo,
        Tone::Auto => Face::Warning,
        Tone::Alarm => Face::DiagnosticError,
    }
}

/// The footers of a frame, as `natives` has them said, in `room`'s allocation. A row with nothing to
/// say, as of a session that is gone, is blank.
pub(in crate::app::draw) fn rows<'a>(
    editor: &Editor,
    natives: Option<&'a NativeSessions>,
    room: &mut Room,
) -> Vec<Row<'a>> {
    let mut rows = reuse(std::mem::take(&mut room.rows));
    rows.extend(natives.into_iter().flat_map(|natives| {
        let windows = editor.frame().windows().iter();
        windows.filter_map(move |(id, _)| row(editor, natives, *id))
    }));
    rows
}

/// The footer of window `id`, if it shows one.
fn row<'a>(editor: &Editor, natives: &'a NativeSessions, id: WindowId) -> Option<Row<'a>> {
    let region = editor.frame().footer(id)?;
    let host = editor.window_by_id(id)?.buffer();
    let footer = natives.footer(host)?;
    // The key is the composer's: none while its place is another's.
    let composer =
        (editor.dock_window(host, DockSlot::Composer)).is_some_and(|id| is_composer(editor, id));
    let key = natives.placeholder().filter(|_| composer);
    let key = key.map_or("", |says| says.cycles.as_str());
    let columns = region.columns.saturating_sub(INDENT.len() + MARGIN);
    let parts = footer.parts(columns, key);
    let dim = |text| Span {
        text,
        face: Face::Prompt,
    };
    let mode = Span {
        text: parts.mode,
        face: face(parts.tone),
    };
    let left = [dim(INDENT), mode, dim(parts.strayed), dim(parts.then)];
    let right = match parts.model_cells {
        0 => 0,
        cells => (cells + MARGIN).min(region.columns),
    };
    Some((id, region, left, ([dim(parts.model)], right)))
}

/// The lines of each of `rows`, in `room`'s allocation.
pub(in crate::app::draw) fn lines<'a>(rows: &'a [Row<'a>], room: &mut Room) -> Vec<Lines<'a>> {
    let mut lines = reuse(std::mem::take(&mut room.lines));
    lines.extend(
        rows.iter()
            .map(|(_, _, left, (right, _))| [[&left[..]], [&right[..]]]),
    );
    lines
}

/// Gives back the lines a frame took of `room`, before the rows they are over.
pub(in crate::app::draw) fn give_back_lines(room: &mut Room, lines: Vec<Lines<'_>>) {
    room.lines = reuse(lines);
}

/// Gives back the rows a frame took of `room`.
pub(in crate::app::draw) fn give_back_rows(room: &mut Room, rows: Vec<Row<'_>>) {
    room.rows = reuse(rows);
}

/// Pushes the panes of `rows`, `lines` their lines: the left's, and the right's if it says anything,
/// side by side over the whole row.
pub(super) fn panes<'a>(rows: &'a [Row<'a>], lines: &'a [Lines<'a>], list: &mut Vec<Pane<'a>>) {
    for ((id, region, _, (_, cells)), [left, right]) in rows.iter().zip(lines) {
        let cells = *cells;
        let beside = region.columns - cells;
        let its = |part, column, columns, line| Pane {
            id: pane_id(*id, part),
            region: CellRect {
                column: region.column + column,
                columns,
                ..*region
            },
            layer: Layer::TILED,
            background: Some(Face::Text),
            panel: None,
            bands: Bands::NONE,
            rules: Rules::NONE,
            content: PaneContent::Lines(line),
        };
        list.push(its(Part::Footer, 0, beside, &left[..]));
        if cells > 0 {
            list.push(its(Part::Model, beside, cells, &right[..]));
        }
    }
}

/// What the first footer of a frame says, its model after it, for the dev socket.
#[cfg(feature = "dev")]
pub(in crate::app::draw) fn said(editor: &Editor, natives: &NativeSessions) -> Option<String> {
    let rows = rows(editor, Some(natives), &mut Room::default());
    let (_, _, left, (right, _)) = rows.first()?;
    let text = |spans: &[Span<'_>]| spans.iter().map(|span| span.text).collect::<String>();
    Some(format!("{}  {}", text(left).trim(), text(right)))
}

#[cfg(test)]
mod tests;
