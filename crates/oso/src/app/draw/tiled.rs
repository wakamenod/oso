//! Each tiled window's part of a frame (ADR 0112): its text, gutter, modeline and divider
//! panes, the cells its text fills, and the pane ids that keep each window's shaped rows
//! from frame to frame.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.
//!
//! The selected window draws as the one window always has. Any other, and every window
//! beneath a prompt, has a hollow cursor, no region, band, flash or match, and an inactive
//! modeline, as the editor's cells and status already say.

use oso_core::editor::{Editor, Gutter, MatchCells, Status};
use oso_core::frame::{CellRect, WindowRegions};
use oso_core::window::{Screen, WindowId};
use oso_render::{
    Bands, CursorStyle, Face, FlashFill, Layer, Pane, PaneContent, PaneId, Span, TextView,
};

/// The first pane id of the windows': the echo area's and the floats' are below it.
const FIRST: u32 = 16;
/// The panes a window draws: its text, gutter, modeline and divider.
const PER_WINDOW: u32 = 4;

/// Which of a window's panes.
#[derive(Clone, Copy)]
pub(super) enum Part {
    Text = 0,
    Gutter = 1,
    Modeline = 2,
    Divider = 3,
}

/// The id of a window's pane, from the window's id, so that a window keeps its shaped rows
/// when another is split or deleted (ADR 0112). Ids come round again only after a billion
/// windows, far apart from any two shown together.
pub(super) fn pane_id(window: WindowId, part: Part) -> PaneId {
    let slots = (u32::MAX - FIRST) / PER_WINDOW;
    let slot = u32::try_from(window.get() % u64::from(slots)).unwrap_or(0);
    PaneId(FIRST + slot * PER_WINDOW + part as u32)
}

/// What a window's text fills and draws besides its rows, kept from frame to frame so that
/// none of it is allocated again: the regions' (ADR 0071), isearch's matches' (ADR 0082), the
/// current line's band and trailing whitespace (ADR 0097), the other cursors' boxes
/// (ADR 0091), its rows in view (ADR 0104) and its gutter's numbers (ADR 0097).
#[derive(Default)]
pub(super) struct WindowCells {
    region: Vec<CellRect>,
    others: Vec<CellRect>,
    matches: MatchCells,
    band: Vec<CellRect>,
    trailing: Vec<CellRect>,
    screen: Screen,
    gutter: Gutter,
}

impl WindowCells {
    /// Fills in what the window `id` draws now.
    fn fill(&mut self, editor: &Editor, id: WindowId) {
        editor.screen_of(id, &mut self.screen);
        let screen = &self.screen;
        editor.region_cells(id, screen, &mut self.region);
        editor.other_cursor_cells(id, screen, &mut self.others);
        editor.isearch_cells(id, screen, &mut self.matches);
        editor.trailing_cells(id, screen, &mut self.trailing);
        editor.gutter_into(id, screen, &mut self.gutter);
        self.band.clear();
        self.band.extend(editor.current_line_cells(id, screen));
    }
}

/// Fills in what every tiled window draws, in the tree's order, into `cells`, which keeps a
/// window's room for the next frame.
pub(super) fn fill(editor: &Editor, cells: &mut Vec<WindowCells>) {
    let windows = editor.frame().windows();
    cells.resize_with(windows.len(), WindowCells::default);
    for (cells, (id, _)) in cells.iter_mut().zip(windows) {
        cells.fill(editor, *id);
    }
}

/// Every tiled window's status, in the tree's order, into `out`, reusing its strings.
pub(in crate::app) fn statuses_into(editor: &Editor, out: &mut Vec<(WindowId, Status)>) {
    let windows = editor.frame().windows();
    out.resize_with(windows.len(), || (windows[0].0, Status::default()));
    for ((id, status), (window, _)) in out.iter_mut().zip(windows) {
        *id = *window;
        editor.status_into(*window, status);
    }
}

/// The status of the selected tiled window, or the one beneath the prompts, among
/// `statuses`, which says the echo area too.
pub(super) fn tiled_status<'a>(
    editor: &Editor,
    statuses: &'a [(WindowId, Status)],
) -> Option<&'a Status> {
    let tiled = editor.tiled().0.id();
    statuses
        .iter()
        .find_map(|(id, status)| (*id == tiled).then_some(status))
}

/// A modeline's spans: warnings first, so that nothing can push them off the row, after
/// `Agent` while the agent channel is on, dim, and bright while an agent is connected
/// (ADR 0085), which only the selected window's says. A name and the rest are shaped apart,
/// so that neither changes how the other reads. A window that is not selected has its
/// modeline in [`Face::ModelineInactive`] (ADR 0112).
fn modeline_spans(status: &Status) -> [Span<'_>; 4] {
    let span = |text, face| Span { text, face };
    let agent = if status.agent_connected {
        Face::Warning
    } else {
        Face::Prompt
    };
    let face = modeline_face(status);
    [
        span(status.agent, agent),
        span(&status.warnings[..], Face::Warning),
        span(&status.name[..], face),
        span(&status.state[..], face),
    ]
}

fn modeline_face(status: &Status) -> Face {
    if status.selected {
        Face::Modeline
    } else {
        Face::ModelineInactive
    }
}

/// The fills behind a window's text, from the back: the current line's band, trailing
/// whitespace, then the current match first, so that it always has room, and the regions
/// over the matches.
fn fills_of(cells: &WindowCells) -> [(Face, &[CellRect]); 5] {
    [
        (Face::CurrentLine, &cells.band[..]),
        (Face::TrailingWhitespace, &cells.trailing[..]),
        (Face::SearchCurrent, &cells.matches.current[..]),
        (Face::SearchMatch, &cells.matches.others[..]),
        (Face::Region, &cells.region[..]),
    ]
}

/// Room for what a frame lists of the tiled windows, empty between frames, which each frame
/// takes at its own lifetime ([`super::reuse`]), so that a frame allocates nothing once warm.
#[derive(Default)]
pub(super) struct Room {
    modelines: Vec<[Span<'static>; 4]>,
    modeline_rows: Vec<[&'static [Span<'static>]; 1]>,
    numbers: Vec<[Span<'static>; 1]>,
    gutters: Vec<&'static [Span<'static>]>,
    fills: Vec<[(Face, &'static [CellRect]); 5]>,
}

/// The status of the window `id`, the `index`th in the tree, among `statuses`, by its id so
/// that a window never takes another's modeline: `empty` if it has none.
fn status_of<'a>(
    statuses: &'a [(WindowId, Status)],
    (index, id): (usize, WindowId),
    empty: &'a Status,
) -> &'a Status {
    let found = statuses
        .get(index)
        .filter(|(other, _)| *other == id)
        .or_else(|| statuses.iter().find(|(other, _)| *other == id));
    found.map_or(empty, |(_, status)| status)
}

/// The spans of every window's modeline and gutter, and the fills behind its text, in
/// `room`'s allocations. A window with no status has `empty`'s modeline.
pub(super) fn spans_of<'a>(
    windows: &[(WindowId, WindowRegions)],
    statuses: &'a [(WindowId, Status)],
    cells: &'a [WindowCells],
    empty: &'a Status,
    room: &mut Room,
) -> Spans<'a> {
    let mut modelines: Vec<[Span<'a>; 4]> = super::reuse(std::mem::take(&mut room.modelines));
    modelines.extend(
        windows
            .iter()
            .enumerate()
            .map(|(index, (id, _))| modeline_spans(status_of(statuses, (index, *id), empty))),
    );
    let mut numbers: Vec<[Span<'a>; 1]> = super::reuse(std::mem::take(&mut room.numbers));
    for window in cells {
        numbers.extend(window.gutter.rows().map(|(text, current)| {
            let face = if current {
                Face::LineNumberCurrent
            } else {
                Face::LineNumber
            };
            [Span { text, face }]
        }));
    }
    let mut fills: Vec<[(Face, &'a [CellRect]); 5]> = super::reuse(std::mem::take(&mut room.fills));
    fills.extend(cells.iter().map(fills_of));
    (modelines, numbers, fills)
}

/// Every window's modeline spans, gutter numbers and fills, as [`spans_of`] makes them.
pub(super) type Spans<'a> = (
    Vec<[Span<'a>; 4]>,
    Vec<[Span<'a>; 1]>,
    Vec<[(Face, &'a [CellRect]); 5]>,
);

/// The rows of every window's modeline and gutter, over the spans [`spans_of`] made.
pub(super) fn rows_of<'a>(
    modelines: &'a [[Span<'a>; 4]],
    numbers: &'a [[Span<'a>; 1]],
    room: &mut Room,
) -> (Vec<[&'a [Span<'a>]; 1]>, Vec<&'a [Span<'a>]>) {
    let mut modeline_rows: Vec<[&'a [Span<'a>]; 1]> =
        super::reuse(std::mem::take(&mut room.modeline_rows));
    modeline_rows.extend(modelines.iter().map(|spans| [&spans[..]]));
    let mut gutters: Vec<&'a [Span<'a>]> = super::reuse(std::mem::take(&mut room.gutters));
    gutters.extend(numbers.iter().map(|row| &row[..]));
    (modeline_rows, gutters)
}

/// Gives back the rows a frame took of `room`, before the spans they are over.
pub(super) fn give_back_rows(
    room: &mut Room,
    (modeline_rows, gutters): (Vec<[&[Span<'_>]; 1]>, Vec<&[Span<'_>]>),
) {
    room.modeline_rows = super::reuse(modeline_rows);
    room.gutters = super::reuse(gutters);
}

/// Gives back the spans and fills a frame took of `room`.
pub(super) fn give_back_spans(room: &mut Room, (modelines, numbers, fills): Spans<'_>) {
    room.modelines = super::reuse(modelines);
    room.numbers = super::reuse(numbers);
    room.fills = super::reuse(fills);
}

/// What [`panes`] reads, per window in the tree's order.
pub(super) struct Drawn<'a> {
    pub(super) cells: &'a [WindowCells],
    pub(super) fills: &'a [[(Face, &'a [CellRect]); 5]],
    pub(super) modelines: &'a [[&'a [Span<'a>]; 1]],
    pub(super) gutters: &'a [&'a [Span<'a>]],
    pub(super) statuses: &'a [(WindowId, Status)],
    /// The cursor flash's strength in this frame, from the app's clock (ADR 0097).
    pub(super) flash: Option<f32>,
}

/// Pushes every tiled window's panes onto `list`: all their texts, the selected one's last
/// among them, then their gutters, modelines and dividers, as later panes come first if a
/// frame runs out of room, so no amount of text can take their place, nor another window's
/// text the selected one's (ADR 0061).
pub(super) fn panes<'a>(editor: &'a Editor, drawn: &Drawn<'a>, list: &mut Vec<Pane<'a>>) {
    let windows = editor.frame().windows();
    let empty = Status::default();
    let selected =
        |index: usize, id: WindowId| status_of(drawn.statuses, (index, id), &empty).selected;
    let pane = |id, region, background, content| Pane {
        id,
        region,
        layer: Layer::TILED,
        background,
        panel: None,
        bands: Bands::NONE,
        content,
    };
    let shown = || windows.iter().zip(drawn.cells).enumerate();
    for last in [false, true] {
        for (index, ((id, regions), cells)) in shown() {
            let selected = selected(index, *id);
            if selected == last {
                let content = text(editor, (*id, selected), cells, drawn, index);
                list.push(pane(pane_id(*id, Part::Text), regions.text, None, content));
            }
        }
    }
    let mut at = 0;
    for ((id, regions), cells) in windows.iter().zip(drawn.cells) {
        let rows = cells.gutter.rows().count();
        let gutter = drawn.gutters.get(at..at + rows).unwrap_or_default();
        at += rows;
        // No band and no fill: the current line's number marks the line there.
        let content = PaneContent::Lines(gutter);
        list.push(pane(
            pane_id(*id, Part::Gutter),
            regions.gutter,
            None,
            content,
        ));
    }
    for (index, ((id, regions), _)) in shown() {
        let modeline = drawn.modelines.get(index).map_or(&[][..], |row| &row[..]);
        let face = modeline_face(status_of(drawn.statuses, (index, *id), &empty));
        let content = PaneContent::Lines(modeline);
        list.push(pane(
            pane_id(*id, Part::Modeline),
            regions.modeline,
            Some(face),
            content,
        ));
    }
    for (id, regions) in windows {
        divider(*id, regions, list);
    }
}

/// A window's divider pane, while a window is to its right (ADR 0112).
fn divider<'a>(id: WindowId, regions: &WindowRegions, list: &mut Vec<Pane<'a>>) {
    if let Some(region) = regions.divider {
        list.push(Pane {
            id: pane_id(id, Part::Divider),
            region,
            layer: Layer::TILED,
            background: Some(Face::Text),
            panel: None,
            bands: Bands::NONE,
            content: PaneContent::Divider,
        });
    }
}

/// The text pane's content of window `id`, which draws as the selected one if its status says
/// so ([`Status::selected`]).
/// Beneath a prompt every window's cursor is hollow and none shows a preedit, as none is
/// selected (ADR 0072).
fn text<'a>(
    editor: &'a Editor,
    (id, selected): (WindowId, bool),
    cells: &'a WindowCells,
    drawn: &Drawn<'a>,
    index: usize,
) -> PaneContent<'a> {
    let Some(window) = editor.window_by_id(id) else {
        return PaneContent::Lines(&[]);
    };
    let buffer = editor.shown(window);
    let display = buffer.display();
    let cursor = window.cursor_point(&display);
    let screen = &cells.screen;
    let flash = drawn.flash.and_then(|strength| {
        let cells = editor.flash_cells(id, screen)?;
        Some(FlashFill { cells, strength })
    });
    PaneContent::Text(TextView {
        display,
        buffer: window.buffer(),
        revision: buffer.revision(),
        screen,
        cursor,
        cursor_style: if selected {
            CursorStyle::Box
        } else {
            CursorStyle::Hollow
        },
        others: &cells.others,
        // Where the screen was laid out with it: the selected window, not beneath a prompt
        // (ADR 0104).
        preedit: editor.preedit().filter(|_| screen.preedit().is_some()),
        fills: drawn.fills.get(index).map_or(&[][..], |fills| &fills[..]),
        whitespace: editor.visual_aids().whitespace,
        flash,
    })
}
