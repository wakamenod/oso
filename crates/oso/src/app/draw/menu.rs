//! The panes of an open menu's float (ADR 0117): its panel, with a chip for each key, and what
//! it shows, a pane for its title, its describe function's line, each column and `q quit`, so
//! that a column's region cuts what it shows and nothing of one reaches another.
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state and the
//! open menu the dispatcher gives it.
//!
//! The layout is worked out again only when what it is worked out from changes: another menu,
//! the tiled area, the prefix argument's width or the notes (ADR 0117's cost). A label and a
//! note go through the same spans as any other text, so bidirectional controls and invisible
//! characters in them are boxed.

use std::fmt::Write as _;

use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::menu::{self, HeadAction, Layout, Notes, Open, Stay};
use oso_core::minibuffer::cells_of;
use oso_render::{Bands, Face, Layer, Pane, PaneContent, PaneId, Panel, Span};

use crate::dispatch::OpenMenu;

/// The base of the menu's panes' ids: none of the prompt's, the echo area's or a window's
/// near the start, and never shown with a prompt's (ADR 0117).
const MENU_PANES: u32 = 1 << 21;
const PANEL: u32 = MENU_PANES;
const TITLE: u32 = MENU_PANES + 1;
const PREFIX: u32 = MENU_PANES + 2;
const LINE: u32 = MENU_PANES + 3;
const QUIT: u32 = MENU_PANES + 4;
const COLUMNS: u32 = MENU_PANES + 8;

/// Spaces between a row's parts.
const SPACES: &str = concat!(
    "                                                                ",
    "                                                                ",
    "                                                                ",
);

/// What a menu's float is worked out from and keeps from frame to frame, so that its layout is
/// worked out again only when that changes and its text is not allocated again.
#[derive(Default)]
pub(super) struct MenuCells {
    /// What the layout was worked out for: the menu shown, the tiled area and the prefix
    /// argument's cells, beside the notes.
    key: Option<(u64, CellRect, usize)>,
    notes: Notes,
    layout: Option<Layout>,
    checks: Vec<Option<bool>>,
    prefix: String,
    /// Each head's keys, as they are written, worked out with the layout.
    keys: Vec<String>,
    /// Each cut column's `… N more`.
    more: Vec<String>,
    /// `╌` as many times as the widest column.
    rule: String,
    /// Each column's group, by its index in the definition.
    groups: Vec<usize>,
    chips: Vec<(CellRect, Face)>,
}

impl MenuCells {
    /// Works out what `menu`'s float shows over `editor`'s tiled area now, if it is drawn:
    /// not under a prompt, which closes it, nor during a macro's replay, which draws it once
    /// the replay ends (ADR 0117, Q7).
    pub(super) fn update(&mut self, editor: &Editor, menu: Option<&OpenMenu<'_>>) -> bool {
        let Some(menu) = menu.filter(|_| editor.prompt().is_none() && !editor.macro_replaying())
        else {
            self.layout = None;
            self.key = None;
            return false;
        };
        let open = menu.open();
        let notes = menu.notes(editor);
        menu.checks_into(editor, &mut self.checks);
        menu.prefix_into(&mut self.prefix);
        let area = editor.frame().tiled();
        let key = (menu.generation(), area, cells_of(&self.prefix));
        if self.key != Some(key) || self.notes != notes || self.layout.is_none() {
            let definition = open.definition();
            self.layout = Some(menu::layout(definition, &notes, &self.checks, key.2, area));
            if self.key.is_none_or(|(generation, ..)| generation != key.0) {
                self.keys.clear();
                self.keys
                    .extend(definition.heads().iter().map(|head| head.keys.to_string()));
            }
            self.key = Some(key);
            self.notes = notes;
            self.groups_of(open);
            self.more_and_rule();
        }
        self.chips_of(open);
        true
    }

    /// Each column's group: the groups with a head shown, in the order of their rows of
    /// columns, as the layout places them.
    fn groups_of(&mut self, open: &Open) {
        let groups = open.definition().groups();
        self.groups.clear();
        self.groups.extend((0..groups.len()).filter(|&group| {
            groups[group]
                .heads()
                .any(|head| !self.notes.get(head).is_some_and(|note| note.hidden))
        }));
        // A stable sort keeps a row's columns in their order.
        self.groups.sort_by_key(|&group| groups[group].row());
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
        for row in layout.columns.iter().flat_map(|column| &column.rows) {
            if unavailable(&self.notes, row.head) {
                continue;
            }
            let Some(head) = definition.heads().get(row.head) else {
                continue;
            };
            self.chips.push((row.key, key_face(open, row.head)));
            let on = matches!(head.action, HeadAction::Switch(_))
                && self.checks.get(row.head).copied().flatten() == Some(true);
            if let Some(check) = row.check.filter(|_| on) {
                // `[x]`, without the space after it.
                let check = CellRect {
                    columns: check.columns.min(3),
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

    /// The default head's row, marked, where no head binds `RET` itself and it is shown.
    fn marked(&self, open: &Open) -> Option<CellRect> {
        let layout = self.layout.as_ref()?;
        let definition = open.definition();
        let ret = self.notes.ret_head.or(definition.ret_head())?;
        let on_ret = "RET"
            .parse()
            .ok()
            .map(|keys: oso_core::key::KeySequence| keys.0);
        if on_ret.is_some_and(|keys| definition.lookup(&keys) != menu::Lookup::Unbound) {
            return None;
        }
        layout
            .columns
            .iter()
            .flat_map(|column| &column.rows)
            .find(|row| row.head == ret)
            .map(|row| row.cells)
    }
}

/// Whether `head` is unavailable now, by `notes`.
fn unavailable(notes: &Notes, head: usize) -> bool {
    notes
        .get(head)
        .is_some_and(|note| note.unavailable.is_some())
}

/// The face of `head`'s key: a stay head's, a switch's or the usual one (ADR 0117, Q8).
fn key_face(open: &Open, head: usize) -> Face {
    let definition = open.definition();
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

/// The spans of each pane of the float, for one frame.
pub(super) struct MenuSpans<'a> {
    title: [Span<'a>; 1],
    prefix: [Span<'a>; 1],
    line: [Span<'a>; 1],
    quit: [Span<'a>; 2],
    /// Each column's region and rows, its title's and rule's first.
    columns: Vec<(CellRect, Vec<Vec<Span<'a>>>)>,
}

impl<'a> MenuSpans<'a> {
    /// The spans of what `cells` worked out for `open`, if it is drawn.
    pub(super) fn of(cells: &'a MenuCells, open: &'a Open) -> Option<Self> {
        let layout = cells.layout.as_ref()?;
        let definition = open.definition();
        let span = |text, face| Span { text, face };
        let mut columns = Vec::with_capacity(layout.columns.len());
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
            let mut lines: Vec<Vec<Span<'a>>> = Vec::new();
            if column.title.is_some() {
                let group = cells
                    .groups
                    .get(index)
                    .and_then(|&g| definition.groups().get(g));
                let title = group.and_then(|group| group.title()).unwrap_or("");
                lines.push(vec![span(title, Face::MenuTitle)]);
            }
            if let Some(rule) = column.rule {
                let end = cells
                    .rule
                    .char_indices()
                    .nth(rule.columns)
                    .map_or(cells.rule.len(), |(at, _)| at);
                lines.push(vec![span(&cells.rule[..end], Face::MenuNote)]);
            }
            for row in &column.rows {
                lines.push(row_spans(cells, open, row));
            }
            if column.more.is_some() {
                let more = cells.more.get(index).map_or("", String::as_str);
                lines.push(vec![span(more, Face::MenuNote)]);
            }
            columns.push((region, lines));
        }
        Some(Self {
            title: [span(definition.title(), Face::MenuTitle)],
            prefix: [span(&cells.prefix, Face::MenuNote)],
            line: [span(
                cells.notes.line.as_deref().map_or("", capped),
                Face::MenuNote,
            )],
            quit: [span("q", Face::MenuKey), span(" quit", Face::MenuNote)],
            columns,
        })
    }

    /// The rows of each pane, as the renderer takes them.
    pub(super) fn lines(&'a self) -> MenuLines<'a> {
        MenuLines {
            title: [&self.title],
            prefix: [&self.prefix],
            line: [&self.line],
            quit: [&self.quit],
            columns: self
                .columns
                .iter()
                .map(|(region, rows)| (*region, rows.iter().map(Vec::as_slice).collect()))
                .collect(),
        }
    }
}

/// A note or a describe function's line, cut to 512 bytes at a character's boundary, as the
/// layout measures it.
fn capped(text: &str) -> &str {
    let mut end = text.len().min(512);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// One head's row: `key  [x] label  note`, each part where the layout put it.
fn row_spans<'a>(cells: &'a MenuCells, open: &'a Open, row: &menu::Row) -> Vec<Span<'a>> {
    let definition = open.definition();
    let Some(head) = definition.heads().get(row.head) else {
        return Vec::new();
    };
    let dim = unavailable(&cells.notes, row.head);
    let mut spans = Vec::with_capacity(7);
    let mut at = row.cells.column;
    let mut put = |column: usize, text: &'a str, face: Face| {
        if column > at {
            spans.push(Span {
                text: spaces(column - at),
                face: Face::Text,
            });
        }
        spans.push(Span { text, face });
        at = column + cells_of(text);
    };
    let key = cells.keys.get(row.head).map_or("", String::as_str);
    put(
        row.key.column,
        key,
        if dim {
            Face::MenuNote
        } else {
            key_face(open, row.head)
        },
    );
    if let Some(check) = row.check {
        let on = cells.checks.get(row.head).copied().flatten() == Some(true);
        let switch = matches!(head.action, HeadAction::Switch(_));
        let (text, face) = match (on, switch) {
            (true, true) => ("[x]", Face::MenuKeySwitch),
            (true, false) => ("[x]", Face::Text),
            (false, true) => ("[ ]", Face::MenuNote),
            (false, false) => ("[ ]", Face::Text),
        };
        put(check.column, text, if dim { Face::MenuNote } else { face });
    }
    let label_face = if dim { Face::MenuNote } else { Face::Text };
    put(row.label.column, &head.label, label_face);
    if let (Some(note), Some(given)) = (row.note, cells.notes.get(row.head)) {
        put(note.column, capped(&given.text), Face::MenuNote);
    }
    spans
}

/// The rows of each pane, as the renderer takes them.
pub(super) struct MenuLines<'a> {
    title: [&'a [Span<'a>]; 1],
    prefix: [&'a [Span<'a>]; 1],
    line: [&'a [Span<'a>]; 1],
    quit: [&'a [Span<'a>]; 1],
    columns: Vec<(CellRect, Vec<&'a [Span<'a>]>)>,
}

/// Adds the float's panes to `list`, after the tiled ones.
pub(super) fn panes<'a>(
    editor: &Editor,
    (cells, open): (&'a MenuCells, &'a Open),
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
    // A completion's panel, at the center of the tiled area (ADR 0117, Q1).
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
            marked: cells.marked(open),
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
    for (id, (region, rows)) in (COLUMNS..).zip(&lines.columns) {
        list.push(content(id, *region, rows));
    }
    if let Some(quit) = layout.quit {
        list.push(content(QUIT, quit, &lines.quit));
    }
}

#[cfg(test)]
mod tests;
