//! An empty composer's placeholder (ADR 0170, as amended): what the composer is for and its keys,
//! or its session's prompt suggestion and the key that takes it (ADR 0165, part 3f), dim, over
//! its first row from the cell after the cursor's. It is drawn over the window and is no text of
//! the buffer's, so nothing that reads the buffer meets it.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.
//!
//! - A suggestion is Claude's text, so it never shows without oso's words after it, whole, in
//!   spans of their own: it is cut for them, or not shown. Alone it could pass for the hint.

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::frame::{CellRect, WindowRegions};
use oso_core::minibuffer::{cells_of, cut_to};
use oso_core::window::{DockSlot, Screen, WindowId};
use oso_render::{Bands, Face, Layer, Pane, PaneContent, Rules, Span};

use super::{Part, WindowCells, pane_id};
use crate::app::draw::reuse;
use crate::app::sessions::{NativeSessions, Placeholder, empty_composer};

/// Just over the tiled panes, as it goes over a composer's text pane, and under every float.
const OVER_TILED: Layer = Layer(Layer::TILED.0 + 1);

/// The fewest cells a suggestion is cut to, its `…` among them: with no room for them, it is not
/// shown.
const MIN_SUGGESTED: usize = 8;

/// A window's placeholder: where it is, and its row's spans, the last of them maybe empty.
pub(in crate::app::draw) type Row<'a> = (WindowId, CellRect, [Span<'a>; 3]);

/// The line of a placeholder: its spans that say something.
pub(in crate::app::draw) type Line<'a> = [&'a [Span<'a>]; 1];

/// Room for the placeholders of a frame and their lines, empty between frames.
#[derive(Default)]
pub(in crate::app::draw) struct Room {
    rows: Vec<Row<'static>>,
    lines: Vec<Line<'static>>,
}

/// The placeholders of a frame whose windows' cells are `cells`, as `natives` has it said, once
/// it is known, in `room`'s allocation.
pub(in crate::app::draw) fn rows<'a>(
    editor: &Editor,
    cells: &[WindowCells],
    natives: Option<&'a NativeSessions>,
    room: &mut Room,
) -> Vec<Row<'a>> {
    let mut rows = reuse(std::mem::take(&mut room.rows));
    let said = natives.and_then(|natives| Some((natives, natives.placeholder()?)));
    if let Some((natives, says)) = said {
        let windows = editor.frame().windows();
        rows.extend(windows.iter().zip(cells).filter_map(|(place, cells)| {
            let (composer, region) = shown(editor, (place.0, &place.1), &cells.screen)?;
            let texts = texts(says, natives.suggested(composer), region.columns)?;
            // Dim on the float's background, which a composer has.
            let span = |text| Span {
                text,
                face: Face::Prompt,
            };
            Some((place.0, region, texts.map(span)))
        }));
    }
    rows
}

/// The line of each of `rows`, in `room`'s allocation.
pub(in crate::app::draw) fn lines<'a>(rows: &'a [Row<'a>], room: &mut Room) -> Vec<Line<'a>> {
    let mut lines = reuse(std::mem::take(&mut room.lines));
    lines.extend(rows.iter().map(|(_, _, spans)| {
        let used = spans.iter().rposition(|span| !span.text.is_empty());
        [&spans[..used.map_or(0, |last| last + 1)]]
    }));
    lines
}

/// Gives back the lines a frame took of `room`, before the rows they are over.
pub(in crate::app::draw) fn give_back_lines(room: &mut Room, lines: Vec<Line<'_>>) {
    room.lines = reuse(lines);
}

/// Gives back the rows a frame took of `room`.
pub(in crate::app::draw) fn give_back_rows(room: &mut Room, rows: Vec<Row<'_>>) {
    room.rows = reuse(rows);
}

/// The composer docked window `id` shows and the cells of its placeholder, over the first row of
/// its `regions`; or none: in no composer's window, or over text, text being composed included.
fn shown(
    editor: &Editor,
    (id, regions): (WindowId, &WindowRegions),
    screen: &Screen,
) -> Option<(BufferId, CellRect)> {
    let window = editor.window_by_id(id)?;
    let composer = matches!(window.docked(), Some((_, DockSlot::Composer)));
    if !composer || screen.preedit().is_some() || !empty_composer(editor.shown(window)) {
        return None;
    }
    let text = regions.text;
    // The cursor, filled or hollow, is in the first cell.
    let region = CellRect {
        column: text.column + 1,
        rows: 1,
        columns: text.columns.saturating_sub(1),
        ..text
    };
    (text.rows > 0 && region.columns > 0).then_some((window.buffer(), region))
}

/// The texts of a placeholder in `columns` cells: what is `suggested`, cut with `…` for the key
/// after it, which is whole; with none, or with no room for both, the longest of oso's forms
/// that fits, else the shortest, cut by its pane.
fn texts<'a>(
    says: &'a Placeholder,
    suggested: Option<&'a str>,
    columns: usize,
) -> Option<[&'a str; 3]> {
    if let Some(text) = suggested {
        let takes = says.takes.as_str();
        let room = columns.saturating_sub(cells_of(takes));
        let (kept, cut) = cut_to(text, room);
        if cut.is_empty() || room >= MIN_SUGGESTED {
            return Some([kept, cut, takes]);
        }
    }
    let fits = says.usual.iter().find(|text| cells_of(text) <= columns);
    (fits.or(says.usual.last())).map(|text| [text.as_str(), "", ""])
}

/// The pane of a placeholder over its window's text, `line` its spans, between its window's `rules`.
pub(super) fn pane<'a>((id, region, _): &Row<'_>, line: &'a Line<'a>, rules: Rules) -> Pane<'a> {
    Pane {
        id: pane_id(*id, Part::Placeholder),
        region: *region,
        layer: OVER_TILED,
        // The current line's band and the flash show behind it, as behind text.
        background: None,
        panel: None,
        bands: Bands::NONE,
        rules,
        content: PaneContent::Lines(line),
    }
}
