//! Each tiled window's part of a frame (ADR 0112): its text, gutter, sign column, modeline and
//! divider panes, the cells its text fills and its diagnostics mark (ADR 0138), and the pane ids
//! that keep each window's shaped rows from frame to frame.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.
//!
//! `*Spaces*` in the side window draws its own bands instead of the current line's
//! (`pane.rs`, ADR 0140).
//!
//! The selected window draws as the one window always has. Any other has a hollow cursor, no
//! region, band, flash or match, and an inactive modeline; but while a docked window is
//! selected, its host has the active one (ADR 0170). Beneath a prompt every window's
//! cursor is hollow and its modeline inactive, but the window the prompt came from keeps its
//! region and band (ADR 0072, ADR 0097). The editor's cells and status say all this but the
//! cursor, which is the window's that keys go to ([`Editor::window`]).

use oso_core::buffer::{BufferId, Mode};
use oso_core::diagnostic::Severity;
use oso_core::editor::{
    DiagnosticCells, Editor, Gutter, JumpCells, MatchCells, ReviewCells, Status,
};
use oso_core::frame::{CellRect, WindowRegions};
use oso_core::window::{Screen, WindowId};
use oso_render::{
    Bands, CursorStyle, Face, FlashFill, GridRow, GridView, Layer, Pane, PaneContent, PaneId, Span,
    TextMarks, TextView,
};

use super::card::CardFills;
use super::spaces::SpacesFills;
use super::transcript::TranscriptFills;
use crate::sessions::list::SpaceBands;
use crate::terminal::hosted::Terminals;

mod labels;
mod modeline;
pub(super) mod placeholder;
pub(in crate::app) use modeline::{statuses_into, tiled_status};

use labels::{label_runs, labels_of};
use modeline::{MODELINE_SPANS, modeline_face, modeline_spans};

/// The first pane id of the windows': the echo area's and the floats' are below it.
const FIRST: u32 = 16;
/// The panes a window may draw: its text, gutter, modeline, divider, sign column and a
/// composer's placeholder.
const PER_WINDOW: u32 = 6;

/// Which of a window's panes.
#[derive(Clone, Copy)]
pub(super) enum Part {
    Text = 0,
    Gutter = 1,
    Modeline = 2,
    Divider = 3,
    Signs = 4,
    Placeholder = 5,
}

/// The id of a window's pane, from the window's id, so that a window keeps its shaped rows
/// when another is split or deleted (ADR 0112). Ids come round again only after 700 million
/// windows have been made, as `WindowId`s are never reused; two shown together would have to
/// be that far apart to share one, which no session reaches.
pub(super) fn pane_id(window: WindowId, part: Part) -> PaneId {
    let slots = (u32::MAX - FIRST) / PER_WINDOW;
    let slot = u32::try_from(window.get() % u64::from(slots)).unwrap_or(0);
    PaneId(FIRST + slot * PER_WINDOW + part as u32)
}

/// What a window's text fills and draws besides its rows, kept from frame to frame so that
/// none of it is allocated again: the regions' (ADR 0071), isearch's matches' (ADR 0082), the
/// current line's band and trailing whitespace (ADR 0097), the other cursors' boxes
/// (ADR 0091), its rows in view (ADR 0104), its gutter's numbers (ADR 0097), a jump's
/// matches and labels (ADR 0116), a review's bands (ADR 0122), a server's diagnostics and
/// the gutter's signs (ADR 0138), dictation's provisional spans (ADR 0175), and a native
/// session's rows' bands (ADR 0165).
#[derive(Default)]
pub(super) struct WindowCells {
    region: Vec<CellRect>,
    others: Vec<CellRect>,
    matches: MatchCells,
    jump: JumpCells,
    band: Vec<CellRect>,
    trailing: Vec<CellRect>,
    provisional: Vec<CellRect>,
    review: ReviewCells,
    /// `*Spaces*`' bands, in the side window (ADR 0140).
    pane: SpacesFills,
    transcript: TranscriptFills,
    card: CardFills,
    screen: Screen,
    gutter: Gutter,
    diagnostics: DiagnosticCells,
    signs: Vec<Option<Face>>,
}

impl WindowCells {
    /// Fills in what the window `id` draws now; `bands` are `*Spaces*`' (ADR 0140).
    fn fill(&mut self, editor: &Editor, bands: &SpaceBands, id: WindowId) {
        editor.screen_of(id, &mut self.screen);
        let screen = &self.screen;
        editor.region_cells(id, screen, &mut self.region);
        editor.other_cursor_cells(id, screen, &mut self.others);
        editor.isearch_cells(id, screen, &mut self.matches);
        // A query-replace's matches in place of isearch's while one is under way (ADR 0113).
        editor.replace_cells(id, screen, &mut self.matches);
        editor.jump_cells(id, screen, &mut self.jump);
        editor.trailing_cells(id, screen, &mut self.trailing);
        editor.dictation_span_cells(id, screen, &mut self.provisional);
        editor.review_cells(id, screen, &mut self.review);
        editor.gutter_into(id, screen, &mut self.gutter);
        editor.diagnostic_cells(id, screen, &mut self.diagnostics);
        self.signs.clear();
        let signs = self.gutter.signs().iter();
        self.signs
            .extend(signs.map(|sign| sign.and_then(severity_face)));
        self.band.clear();
        self.band.extend(editor.current_line_cells(id, screen));
        self.pane
            .fill((editor, bands), (id, screen), &mut self.band);
        self.transcript.fill(editor, (id, screen));
        self.card.fill(editor, (id, screen), &mut self.band);
    }
}

/// Fills in what every tiled window draws, in the tree's order, into `cells`, which keeps a
/// window's room for the next frame; `bands` are `*Spaces*`' (ADR 0140).
pub(super) fn fill(editor: &Editor, bands: &SpaceBands, cells: &mut Vec<WindowCells>) {
    let windows = editor.frame().windows();
    cells.resize_with(windows.len(), WindowCells::default);
    for (cells, (id, _)) in cells.iter_mut().zip(windows) {
        cells.fill(editor, bands, *id);
    }
}

/// The rows the selected tiled window shows this frame, as `fill` laid them out.
pub(super) fn selected_screen<'a>(editor: &Editor, cells: &'a [WindowCells]) -> Option<&'a Screen> {
    let selected = editor.window().id();
    let windows = editor.frame().windows();
    let index = windows.iter().position(|(id, _)| *id == selected)?;
    cells.get(index).map(|cells| &cells.screen)
}

/// The face a diagnostic of `severity` is drawn in, its line, its sign and its count: none for
/// a hint (ADR 0138, Q16).
pub(super) fn severity_face(severity: Severity) -> Option<Face> {
    match severity {
        Severity::Error => Some(Face::DiagnosticError),
        Severity::Warning => Some(Face::DiagnosticWarning),
        Severity::Information => Some(Face::DiagnosticInfo),
        Severity::Hint => None,
    }
}

/// A window's diagnostics' wavy lines, which do not meet, the most severe first, so that it keeps
/// its room if a frame runs out of it (ADR 0138).
fn underlines_of(cells: &WindowCells) -> [(Face, &[CellRect]); UNDERLINES] {
    let found = &cells.diagnostics;
    [
        (Face::DiagnosticError, &found.error[..]),
        (Face::DiagnosticWarning, &found.warning[..]),
        (Face::DiagnosticInfo, &found.information[..]),
    ]
}

/// How many lists of cells a window draws wavy lines under.
const UNDERLINES: usize = 3;

/// What a window draws behind its glyphs: its fills, and its diagnostics' wavy lines.
pub(super) struct Behind<'a> {
    fills: [(Face, &'a [CellRect]); FILLS],
    underlines: [(Face, &'a [CellRect]); UNDERLINES],
}

/// The fills behind a window's text, from the back: `*Spaces*`' bands and the lit beat of its
/// blink, in the side window only (ADR 0140), the current line's band, a native session's rows'
/// bands (ADR 0165), a review's bands
/// (added and removed lines, a pane's filler rows, their refined words, file headers and
/// comment cards), trailing
/// whitespace, a dictation's text while Claude fixes it (ADR 0175), then the current match first, so that it always has room, a jump's matches as
/// isearch's others (ADR 0116), and the regions over the matches.
fn fills_of(cells: &WindowCells) -> [(Face, &[CellRect]); FILLS] {
    [
        // `*Spaces*`' bands, in the side window only (ADR 0140).
        (Face::SessionHeader, &cells.pane.sections[..]),
        (Face::SessionFrontTint, &cells.pane.tint[..]),
        (Face::SessionPoint, &cells.pane.point[..]),
        (Face::SessionPointBar, &cells.pane.bar[..]),
        // A waiting row's mark and name on the blink's lit beat, over the point's band.
        (Face::SessionAlertLit, &cells.pane.lit[..]),
        (Face::CurrentLine, &cells.band[..]),
        // A native session's rows', as a review's (ADR 0165).
        (Face::SessionHeader, &cells.transcript.prompts[..]),
        (Face::DiffAdded, &cells.transcript.added[..]),
        (Face::DiffRemoved, &cells.transcript.removed[..]),
        // A permission card's title and keys (ADR 0180).
        (Face::CardTitle, &cells.card.title[..]),
        (Face::MenuKey, &cells.card.keys[..]),
        (Face::DiffAdded, &cells.review.added[..]),
        (Face::DiffRemoved, &cells.review.removed[..]),
        (Face::DiffFiller, &cells.review.fillers[..]),
        (Face::DiffRefineAdded, &cells.review.refine_added[..]),
        (Face::DiffRefineRemoved, &cells.review.refine_removed[..]),
        (Face::DiffFileHeader, &cells.review.headers[..]),
        (Face::ReviewCommentPerson, &cells.review.cards[..]),
        (Face::TrailingWhitespace, &cells.trailing[..]),
        (Face::DictationProvisional, &cells.provisional[..]),
        (Face::SearchCurrent, &cells.matches.current[..]),
        (Face::SearchMatch, &cells.matches.others[..]),
        (Face::SearchMatch, &cells.jump.matches[..]),
        (Face::Region, &cells.region[..]),
    ]
}

/// How many lists of cells a window fills behind its text.
const FILLS: usize = 24;

/// Room for what a frame lists of the tiled windows, empty between frames, which each frame
/// takes at its own lifetime ([`super::reuse`]), so that a frame allocates nothing once warm.
#[derive(Default)]
pub(super) struct Room {
    modelines: Vec<[Span<'static>; MODELINE_SPANS]>,
    modeline_rows: Vec<[&'static [Span<'static>]; 1]>,
    numbers: Vec<[Span<'static>; 1]>,
    gutters: Vec<&'static [Span<'static>]>,
    behind: Vec<Behind<'static>>,
    labels: Vec<(CellRect, Span<'static>)>,
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
    let mut modelines: Vec<[Span<'a>; MODELINE_SPANS]> =
        super::reuse(std::mem::take(&mut room.modelines));
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
    let mut behind: Vec<Behind<'a>> = super::reuse(std::mem::take(&mut room.behind));
    behind.extend(cells.iter().map(|cells| Behind {
        fills: fills_of(cells),
        underlines: underlines_of(cells),
    }));
    let mut labels: Vec<(CellRect, Span<'a>)> = super::reuse(std::mem::take(&mut room.labels));
    for window in cells {
        labels_of(window, &mut labels);
    }
    (modelines, numbers, behind, labels)
}

/// Every window's modeline spans, gutter numbers and fills, as [`spans_of`] makes them.
pub(super) type Spans<'a> = (
    Vec<[Span<'a>; MODELINE_SPANS]>,
    Vec<[Span<'a>; 1]>,
    Vec<Behind<'a>>,
    Vec<(CellRect, Span<'a>)>,
);

/// The rows of every window's modeline and gutter, over the spans [`spans_of`] made.
pub(super) fn rows_of<'a>(
    modelines: &'a [[Span<'a>; MODELINE_SPANS]],
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
pub(super) fn give_back_spans(room: &mut Room, (modelines, numbers, behind, labels): Spans<'_>) {
    room.modelines = super::reuse(modelines);
    room.numbers = super::reuse(numbers);
    room.behind = super::reuse(behind);
    room.labels = super::reuse(labels);
}

/// The terminals a frame shows (ADR 0120): their rows, each terminal's once, and which of
/// them each terminal's buffer has.
pub(super) struct Grids<'a> {
    pub(super) terminals: &'a Terminals,
    pub(super) rows: &'a [GridRow<'a>],
    pub(super) shown: &'a [(BufferId, std::ops::Range<usize>)],
}

impl<'a> Grids<'a> {
    pub(super) fn new(
        terminals: &'a Terminals,
        rows: &'a [GridRow<'a>],
        shown: &'a [(BufferId, std::ops::Range<usize>)],
    ) -> Grids<'a> {
        Grids {
            terminals,
            rows,
            shown,
        }
    }
}

/// What [`panes`] reads, per window in the tree's order.
pub(super) struct Drawn<'a> {
    pub(super) terminals: Grids<'a>,
    pub(super) cells: &'a [WindowCells],
    /// What each window draws behind its glyphs.
    pub(super) behind: &'a [Behind<'a>],
    /// Every window's jump labels, one window's after another's in the tree's order.
    pub(super) labels: &'a [(CellRect, Span<'a>)],
    pub(super) modelines: &'a [[&'a [Span<'a>]; 1]],
    pub(super) gutters: &'a [&'a [Span<'a>]],
    pub(super) statuses: &'a [(WindowId, Status)],
    /// An empty composer's placeholder, the whole of it first, or none (ADR 0170).
    pub(super) placeholder: &'a [&'a [Span<'a>]],
    /// The cursor flash's strength in this frame, from the app's clock (ADR 0097).
    pub(super) flash: Option<f32>,
}

/// Pushes every tiled window's panes onto `list`: all their texts, the selected one's last
/// among them, then their sign columns and gutters, modelines and dividers, as later panes come
/// first if a frame runs out of room, so no amount of text can take their place, nor another
/// window's text the selected one's (ADR 0061).
pub(super) fn panes<'a>(editor: &'a Editor, drawn: &Drawn<'a>, list: &mut Vec<Pane<'a>>) {
    let windows = editor.frame().windows();
    let empty = Status::default();
    // Where keys go, docked or not: none of these beneath a prompt. Not the window whose
    // modeline is active, which is a selected dock's host (ADR 0170).
    let selected = editor.window().id();
    // A docked window's text is drawn over the float's face, as a prompt's answer is, so that
    // the composer reads as a place to type.
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
    // The docked windows of this frame, found once (ADR 0170).
    let docked: Vec<WindowId> = (editor.windows())
        .filter(|window| window.docked().is_some())
        .map(oso_core::window::Window::id)
        .collect();
    let docked_face = |id: WindowId| super::card::dock_face(editor, id);
    // The selected window's text last, or the text of the one beneath the prompts.
    let tiled = editor.tiled().0.id();
    for last in [false, true] {
        let mut runs = 0;
        for (index, ((id, regions), cells)) in shown() {
            let labels = (runs..runs + label_runs(cells), index);
            runs = labels.0.end;
            if (*id == tiled) == last {
                let content = text(editor, (*id, *id == selected), cells, drawn, labels);
                let background = docked_face(*id);
                list.push(pane(
                    pane_id(*id, Part::Text),
                    regions.text,
                    background,
                    content,
                ));
            }
        }
    }
    list.extend(shown().filter_map(|(_, ((id, regions), cells))| {
        let place = (*id, regions);
        placeholder::pane(editor, place, &cells.screen, drawn.placeholder)
    }));
    let mut at = 0;
    for ((id, regions), cells) in windows.iter().zip(drawn.cells) {
        let rows = cells.gutter.rows().count();
        let gutter = drawn.gutters.get(at..at + rows).unwrap_or_default();
        at += rows;
        // The sign column at the gutter's left, while a server follows the buffer (ADR 0138).
        let (signs, numbers) = split_gutter(regions.gutter, !cells.signs.is_empty());
        if let Some(signs) = signs {
            let content = PaneContent::Signs(&cells.signs);
            list.push(pane(pane_id(*id, Part::Signs), signs, None, content));
        }
        // No band and no fill: the current line's number marks the line there.
        let content = PaneContent::Lines(gutter);
        list.push(pane(pane_id(*id, Part::Gutter), numbers, None, content));
    }
    // A docked window has no modeline: its host's is under it (ADR 0170).
    for (index, ((id, regions), _)) in shown().filter(|(_, ((id, _), _))| !docked.contains(id)) {
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

/// A gutter's sign column, its first cell if it has `signs`, and the rest, for the numbers.
fn split_gutter(gutter: CellRect, signs: bool) -> (Option<CellRect>, CellRect) {
    if !signs || gutter.columns == 0 {
        return (None, gutter);
    }
    let column = CellRect {
        columns: 1,
        ..gutter
    };
    let rest = CellRect {
        column: gutter.column + 1,
        columns: gutter.columns - 1,
        ..gutter
    };
    (Some(column), rest)
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

/// The text pane's content of window `id`, which draws as the one keys go to if `selected`.
/// Beneath a prompt every window's cursor is hollow and none shows a preedit, as none is
/// selected (ADR 0072).
fn text<'a>(
    editor: &'a Editor,
    (id, selected): (WindowId, bool),
    cells: &'a WindowCells,
    drawn: &Drawn<'a>,
    (labels, index): (std::ops::Range<usize>, usize),
) -> PaneContent<'a> {
    let Some(window) = editor.window_by_id(id) else {
        return PaneContent::Lines(&[]);
    };
    let buffer = editor.shown(window);
    if buffer.is_terminal() {
        return grid(editor, (window, selected), &drawn.terminals);
    }
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
        // `*Spaces*` in the side window marks point's row with the band (ADR 0131).
        cursor_style: if editor.side_window() == Some(id) && buffer.mode() == Mode::Sessions {
            CursorStyle::None
        } else if selected {
            CursorStyle::Box
        } else {
            CursorStyle::Hollow
        },
        others: &cells.others,
        // Where the screen was laid out with it: the selected window, not beneath a prompt
        // (ADR 0104).
        preedit: editor.preedit().filter(|_| screen.preedit().is_some()),
        fills: drawn
            .behind
            .get(index)
            .map_or(&[][..], |behind| &behind.fills[..]),
        labels: drawn.labels.get(labels).unwrap_or_default(),
        whitespace: editor.visual_aids().whitespace,
        flash,
        // None while font-lock is off, and in a buffer that may hold secrets (ADR 0111).
        highlights: buffer.highlights(),
        marks: TextMarks {
            underlines: drawn
                .behind
                .get(index)
                .map_or(&[][..], |behind| &behind.underlines[..]),
            faded: &cells.diagnostics.unnecessary,
            struck: &cells.diagnostics.deprecated,
        },
    })
}

/// The text pane's content of a window that shows a terminal: its grid, as its thread last
/// sent it, with the cursor as the program set it, hollow in a window not selected, and the
/// preedit at the cursor in the selected one (ADR 0120). Nothing of the buffer's own text,
/// fills or flash.
fn grid<'a>(
    editor: &'a Editor,
    (window, selected): (&oso_core::window::Window, bool),
    grids: &Grids<'a>,
) -> PaneContent<'a> {
    let buffer = window.buffer();
    let rows = grids
        .shown
        .iter()
        .find(|(shown, _)| *shown == buffer)
        .and_then(|(_, range)| grids.rows.get(range.clone()))
        .unwrap_or_default();
    let hosted = grids.terminals.get(buffer);
    let cursor = hosted.and_then(|hosted| hosted.grid.cursor(selected));
    let preedit = editor
        .preedit()
        .filter(|_| selected && editor.prompt().is_none());
    // At the program's cursor cell, also while it hides the cursor, as `claude`'s input does.
    let preedit_at = hosted.map_or((0, 0), |hosted| hosted.grid.cursor_cell());
    PaneContent::Grid(GridView {
        id: buffer.get(),
        rows,
        cursor,
        preedit,
        preedit_at,
    })
}
