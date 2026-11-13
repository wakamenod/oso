//! The panes of an open menu's float (ADR 0117): its panel, with a chip for each key, and what
//! it shows, a pane for its title, its describe function's line, each column and `q quit`, so
//! that a column's region cuts what it shows and nothing of one reaches another.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state and the
//! open menu the dispatcher gives it.
//!
//! The layout is worked out again only when what it is worked out from changes: another menu,
//! the tiled area, the prefix argument's width, or the cells and hidden heads of the notes
//! (ADR 0117's cost). A note whose text changes but not its cells is drawn in the same place.
//! A label and a note go through the same spans as any other text, so bidirectional controls
//! and invisible characters in them are boxed.

use std::fmt::Write as _;
use std::ops::Range;

use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::menu::{self, Definition, HeadAction, Layout, Open, Stay};
use oso_core::minibuffer::cells_of;
use oso_render::{Bands, Face, Layer, Pane, PaneContent, PaneId, Panel, Span};

use super::reuse;
use crate::dispatch::{Described, HeadNow, OpenMenu};

/// The base of the menu's panes' ids: none of the prompt's, the echo area's or a window's
/// near the start, and never shown with a prompt's (ADR 0117).
const MENU_PANES: u32 = 1 << 21;
const PANEL: u32 = MENU_PANES;
const TITLE: u32 = MENU_PANES + 1;
const PREFIX: u32 = MENU_PANES + 2;
const LINE: u32 = MENU_PANES + 3;
const QUIT: u32 = MENU_PANES + 4;
/// The first column's id: each column's is this and its index in the layout.
const COLUMNS: u32 = MENU_PANES + 8;

/// Spaces between a row's parts.
const SPACES: &str = concat!(
    "                                                                ",
    "                                                                ",
    "                                                                ",
);

/// The cells of `[x]` and `[ ]`, without the space after them.
const CHECK: usize = 3;

/// What a menu's float is worked out from and keeps from frame to frame, so that its layout is
/// worked out again only when that changes and its text is not allocated again.
#[derive(Default)]
pub(super) struct MenuCells {
    /// What the layout was worked out for: the menu shown, the tiled area and the prefix
    /// argument's cells, beside the notes' shape.
    layout_for: Option<(u64, CellRect, usize)>,
    /// The notes of this frame, and those of the frame before, for their room.
    notes: Described,
    before: Described,
    /// What the layout took from the notes, and room for the next frame's.
    shape: Shape,
    next_shape: Shape,
    layout: Option<Layout>,
    checks: Vec<Option<bool>>,
    prefix: String,
    /// Each head's keys, as they are written, worked out with the layout.
    keys: Vec<String>,
    /// Each cut column's `… N more`.
    more: Vec<String>,
    /// `╌` as many times as the widest column.
    rule: String,
    chips: Vec<(CellRect, Face)>,
    /// The row of the head `RET` runs, where it is the default head and shown.
    marked: Option<CellRect>,
    /// Room for each frame's spans, empty between frames.
    room: Room,
}

/// What a layout takes from the notes: each head's note's cells and whether it is hidden, the
/// line, and the head marked.
#[derive(Default, PartialEq, Eq)]
struct Shape {
    heads: Vec<(usize, bool)>,
    line: bool,
    ret_head: Option<usize>,
}

impl MenuCells {
    /// Works out what `menu`'s float shows over `editor`'s tiled area now, if it is drawn:
    /// not under a prompt, which closes it, nor during a macro's replay, which draws it once
    /// the replay ends (ADR 0117, Q19).
    pub(super) fn update(&mut self, editor: &Editor, menu: Option<&OpenMenu<'_>>) -> bool {
        let Some(menu) = menu.filter(|_| editor.prompt().is_none() && !editor.macro_replaying())
        else {
            self.layout = None;
            self.layout_for = None;
            return false;
        };
        let open = menu.open();
        let definition = open.definition();
        std::mem::swap(&mut self.notes, &mut self.before);
        menu.notes_into(editor, &mut self.notes);
        menu.checks_into(editor, &mut self.checks);
        menu.prefix_into(&mut self.prefix);
        let area = editor.frame().tiled();
        let layout_for = (menu.generation(), area, cells_of(&self.prefix));
        let other_menu = self
            .layout_for
            .is_none_or(|(generation, ..)| generation != layout_for.0);
        let reshaped =
            (other_menu || self.notes.notes() != self.before.notes()) && self.reshaped(definition);
        if self.layout_for != Some(layout_for) || reshaped || self.layout.is_none() {
            let layout = menu::layout(
                definition,
                self.notes.notes(),
                &self.checks,
                layout_for.2,
                area,
            );
            if other_menu {
                self.keys.clear();
                self.keys
                    .extend(definition.heads().iter().map(|head| head.keys.to_string()));
            }
            self.marked = self.notes.marked_head(definition).and_then(|marked| {
                rows(&layout)
                    .find(|row| row.head == marked)
                    .map(|row| row.cells)
            });
            self.layout = Some(layout);
            self.layout_for = Some(layout_for);
            self.more_and_rule();
        }
        self.chips_of(open);
        true
    }

    /// Works out the notes' shape, and whether it differs from the one the layout took.
    fn reshaped(&mut self, definition: &Definition) -> bool {
        let next = &mut self.next_shape;
        next.heads.clear();
        next.heads.extend((0..definition.heads().len()).map(|head| {
            self.notes
                .get(head)
                .map_or((0, false), |note| (cells_of(&note.text), note.hidden))
        }));
        next.line = self.notes.notes().line.is_some();
        next.ret_head = self.notes.notes().ret_head;
        let reshaped = *next != self.shape;
        std::mem::swap(&mut self.shape, &mut self.next_shape);
        reshaped
    }

    /// Writes each cut column's `… N more` and the rule, for the layout there is.
    fn more_and_rule(&mut self) {
        let Some(layout) = &self.layout else {
            return;
        };
        self.more.clear();
        let mut widest = 0;
        for column in &layout.columns {
            let mut more = String::new();
            if let Some((cut, _)) = column.more {
                let _ = write!(more, "… {cut} more");
            }
            self.more.push(more);
            widest = widest.max(column.rule.map_or(0, |rule| rule.columns));
        }
        let have = self.rule.chars().count();
        if have < widest {
            self.rule.extend(std::iter::repeat_n('╌', widest - have));
        }
    }

    /// The chips of the keys shown, by kind, and of the switches that are on.
    fn chips_of(&mut self, open: &Open) {
        self.chips.clear();
        let Some(layout) = &self.layout else {
            return;
        };
        let definition = open.definition();
        for row in rows(layout) {
            if matches!(self.notes.now(row.head), HeadNow::Unavailable(_)) {
                continue;
            }
            let Some(head) = definition.heads().get(row.head) else {
                continue;
            };
            self.chips.push((row.key, key_face(definition, row.head)));
            let on = matches!(head.action, HeadAction::Switch(_))
                && self.checks.get(row.head).copied().flatten() == Some(true);
            if let Some(check) = row.check.filter(|_| on) {
                let check = CellRect {
                    columns: check.columns.min(CHECK),
                    ..check
                };
                self.chips.push((check, Face::MenuKeySwitch));
            }
        }
        if let Some(quit) = layout.quit {
            let q = CellRect {
                columns: quit.columns.min(1),
                ..quit
            };
            self.chips.push((q, Face::MenuKey));
        }
    }
}

/// Every shown head's row, column after column.
fn rows(layout: &Layout) -> impl Iterator<Item = &menu::Row> {
    layout.columns.iter().flat_map(|column| &column.rows)
}

/// The face of `head`'s key: a stay head's, a switch's or the usual one (ADR 0117, Q8).
fn key_face(definition: &Definition, head: usize) -> Face {
    match definition.heads().get(head).map(|head| &head.action) {
        Some(HeadAction::Switch(_)) => Face::MenuKeySwitch,
        Some(HeadAction::Command(_)) if definition.stay(head) == Stay::Stay => Face::MenuKeyStay,
        _ => Face::MenuKey,
    }
}

/// `n` spaces, at most as many as [`SPACES`] holds.
fn spaces(n: usize) -> &'static str {
    &SPACES[..n.min(SPACES.len())]
}

/// Room for a frame's spans and rows, empty between frames.
#[derive(Default)]
pub(super) struct Room {
    spans: Vec<Span<'static>>,
    rows: Vec<Range<usize>>,
    columns: Vec<(u32, CellRect, Range<usize>)>,
    slices: Vec<&'static [Span<'static>]>,
}

impl MenuCells {
    /// The room for this frame's spans, to give back after it.
    pub(super) fn take_room(&mut self) -> Room {
        std::mem::take(&mut self.room)
    }

    /// Takes back the room a frame's spans and rows took.
    pub(super) fn give_back(&mut self, room: Room) {
        self.room = room;
    }
}

/// `room`, with the room of a frame's rows back in it.
pub(super) fn lines_back(mut room: Room, lines: Option<MenuLines<'_>>) -> Room {
    if let Some(lines) = lines {
        room.slices = reuse(lines.rows);
    }
    room
}

/// `room`, with the room of a frame's spans back in it.
pub(super) fn spans_back(mut room: Room, spans: Option<MenuSpans<'_>>) -> Room {
    if let Some(spans) = spans {
        room.spans = reuse(spans.spans);
        room.rows = reuse(spans.rows);
        room.columns = reuse(spans.columns);
    }
    room
}

/// The spans of each pane of the float, for one frame.
pub(super) struct MenuSpans<'a> {
    title: [Span<'a>; 1],
    prefix: [Span<'a>; 1],
    line: [Span<'a>; 1],
    quit: [Span<'a>; 2],
    /// Every column's rows' spans, one after another.
    spans: Vec<Span<'a>>,
    /// Each row's spans in `spans`.
    rows: Vec<Range<usize>>,
    /// Each column's pane id, region and rows in `rows`: its title's and rule's first.
    columns: Vec<(u32, CellRect, Range<usize>)>,
}

impl<'a> MenuSpans<'a> {
    /// The spans of what `cells` worked out for `open`, if it is drawn, in `room`'s buffers.
    pub(super) fn of(cells: &'a MenuCells, open: &'a Open, room: &mut Room) -> Option<Self> {
        let layout = cells.layout.as_ref()?;
        let definition = open.definition();
        let span = |text, face| Span { text, face };
        let mut spans: Vec<Span<'a>> = reuse(std::mem::take(&mut room.spans));
        let mut rows = std::mem::take(&mut room.rows);
        let mut columns = std::mem::take(&mut room.columns);
        rows.clear();
        columns.clear();
        let mut first_row = 0;
        for (index, column) in layout.columns.iter().enumerate() {
            let rects = column
                .title
                .into_iter()
                .chain(column.rule)
                .chain(column.rows.iter().map(|row| row.cells))
                .chain(column.more.map(|(_, at)| at));
            let Some(region) = rects.reduce(|a, b| CellRect {
                rows: b.row + b.rows - a.row,
                columns: a.columns.max(b.columns),
                ..a
            }) else {
                continue;
            };
            if column.title.is_some() {
                let group = definition.groups().get(column.group);
                let title = group.and_then(|group| group.title()).unwrap_or("");
                let start = spans.len();
                spans.push(span(title, Face::MenuTitle));
                rows.push(start..spans.len());
            }
            if let Some(rule) = column.rule {
                let end = cells
                    .rule
                    .char_indices()
                    .nth(rule.columns)
                    .map_or(cells.rule.len(), |(at, _)| at);
                let start = spans.len();
                spans.push(span(&cells.rule[..end], Face::MenuNote));
                rows.push(start..spans.len());
            }
            for row in &column.rows {
                let start = spans.len();
                row_spans(cells, definition, row, &mut spans);
                rows.push(start..spans.len());
            }
            if column.more.is_some() {
                let more = cells.more.get(index).map_or("", String::as_str);
                let start = spans.len();
                spans.push(span(more, Face::MenuNote));
                rows.push(start..spans.len());
            }
            let id = COLUMNS.saturating_add(u32::try_from(index).unwrap_or(u32::MAX));
            let last_row = rows.len();
            columns.push((id, region, first_row..last_row));
            first_row = last_row;
        }
        let notes = cells.notes.notes();
        Some(Self {
            title: [span(definition.title(), Face::MenuTitle)],
            prefix: [span(&cells.prefix, Face::MenuNote)],
            line: [span(notes.line.as_deref().unwrap_or(""), Face::MenuNote)],
            quit: [span("q", Face::MenuKey), span(" quit", Face::MenuNote)],
            spans,
            rows,
            columns,
        })
    }

    /// The rows of each pane, as the renderer takes them, in `room`'s buffer.
    pub(super) fn lines(&'a self, room: &mut Room) -> MenuLines<'a> {
        let mut rows: Vec<&'a [Span<'a>]> = reuse(std::mem::take(&mut room.slices));
        rows.extend(self.rows.iter().map(|range| &self.spans[range.clone()]));
        MenuLines {
            title: [&self.title],
            prefix: [&self.prefix],
            line: [&self.line],
            quit: [&self.quit],
            rows,
            columns: &self.columns,
        }
    }
}

/// One head's row: `key  [x] label  note`, each part where the layout put it, onto `spans`.
fn row_spans<'a>(
    cells: &'a MenuCells,
    definition: &'a Definition,
    row: &menu::Row,
    spans: &mut Vec<Span<'a>>,
) {
    let Some(head) = definition.heads().get(row.head) else {
        return;
    };
    let dim = matches!(cells.notes.now(row.head), HeadNow::Unavailable(_));
    let mut at = row.cells.column;
    // Each part's cells are the layout's, measured as it worked them out.
    let mut put = |part: CellRect, text: &'a str, face: Face| {
        if part.column > at {
            spans.push(Span {
                text: spaces(part.column - at),
                face: Face::Text,
            });
        }
        spans.push(Span { text, face });
        at = part.column + part.columns;
    };
    let key = cells.keys.get(row.head).map_or("", String::as_str);
    let key_face = if dim {
        Face::MenuNote
    } else {
        key_face(definition, row.head)
    };
    put(row.key, key, key_face);
    if let Some(check) = row.check {
        let on = cells.checks.get(row.head).copied().flatten() == Some(true);
        let switch = matches!(head.action, HeadAction::Switch(_));
        let (text, face) = match (on, switch) {
            (true, true) => ("[x]", Face::MenuKeySwitch),
            (true, false) => ("[x]", Face::Text),
            (false, true) => ("[ ]", Face::MenuNote),
            (false, false) => ("[ ]", Face::Text),
        };
        let check = CellRect {
            columns: check.columns.min(CHECK),
            ..check
        };
        put(check, text, if dim { Face::MenuNote } else { face });
    }
    let label_face = if dim { Face::MenuNote } else { Face::Text };
    put(row.label, &head.label, label_face);
    if let (Some(note), Some(given)) = (row.note, cells.notes.get(row.head)) {
        put(note, &given.text, Face::MenuNote);
    }
}

/// The rows of each pane, as the renderer takes them.
pub(super) struct MenuLines<'a> {
    title: [&'a [Span<'a>]; 1],
    prefix: [&'a [Span<'a>]; 1],
    line: [&'a [Span<'a>]; 1],
    quit: [&'a [Span<'a>]; 1],
    /// Every column's rows, one after another.
    rows: Vec<&'a [Span<'a>]>,
    /// Each column's pane id, region and rows in `rows`.
    columns: &'a [(u32, CellRect, Range<usize>)],
}

/// Adds the float's panes to `list`, after the tiled ones.
pub(super) fn panes<'a>(
    editor: &Editor,
    cells: &'a MenuCells,
    lines: &'a MenuLines<'a>,
    list: &mut Vec<Pane<'a>>,
) {
    let Some(layout) = cells.layout.as_ref() else {
        return;
    };
    if layout.inner.rows == 0 || layout.inner.columns == 0 {
        return;
    }
    let content = |id, region: CellRect, rows: &'a [&'a [Span<'a>]]| Pane {
        id: PaneId(id),
        region,
        layer: Layer::FLOAT_CONTENT,
        background: None,
        panel: None,
        bands: Bands::NONE,
        content: PaneContent::Lines(rows),
    };
    // A completion's panel, at the center of the tiled area (ADR 0117 step 2).
    list.push(Pane {
        id: PaneId(PANEL),
        region: editor.frame().tiled(),
        layer: Layer::FLOAT,
        background: None,
        panel: Some(Panel {
            cells: layout.outer,
            field: None,
            field_room: CellRect {
                rows: 0,
                columns: 0,
                ..layout.outer
            },
            keys: &cells.chips,
            selected: None,
            marked: cells.marked,
            border: true,
        }),
        bands: Bands::NONE,
        content: PaneContent::Lines(&[]),
    });
    if layout.title.columns > 0 {
        list.push(content(TITLE, layout.title, &lines.title));
    }
    if let Some(prefix) = layout.prefix {
        list.push(content(PREFIX, prefix, &lines.prefix));
    }
    if let Some(line) = layout.line {
        list.push(content(LINE, line, &lines.line));
    }
    for (id, region, rows) in lines.columns {
        list.push(content(*id, *region, &lines.rows[rows.clone()]));
    }
    if let Some(quit) = layout.quit {
        list.push(content(QUIT, quit, &lines.quit));
    }
}

#[cfg(test)]
mod tests;
