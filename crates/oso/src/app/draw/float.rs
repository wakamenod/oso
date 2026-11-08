//! The panes of an open prompt's float: its panel, what it says, the answer or a
//! question's keys, a message, and a list's candidates and count (ADR 0072, ADR 0075,
//! ADR 0076).
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.
//!
//! The float's panel, with its field and key chips, is one pane in [`Layer::FLOAT`], over
//! the tiled panes, and what it shows is in [`Layer::FLOAT_CONTENT`], in panes that never
//! overlap one another. A passphrase shows one `*` per character, and nothing of it reaches a
//! pane.

use std::ops::Range;

use oso_core::complete::Candidates;
use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::minibuffer::{self, Kind, PromptLayout, cells_of};
use oso_render::{
    Bands, CursorStyle, Face, Layer, Masked, Pane, PaneContent, PaneId, Panel, Span, TextView,
};

/// The most panes a float takes.
pub(super) const MAX_PANES: usize = 7;

/// What the innermost prompt shows, worked out for one frame.
pub(super) struct Parts<'a> {
    layout: &'a PromptLayout,
    label: &'a str,
    kind: Kind,
    message: Option<&'a str>,
    /// For a passphrase: one `*` per character in view, and the cursor's column among them.
    mask: Option<(String, usize)>,
    /// For a prompt that completes: its candidates, and its count, such as `3/57`.
    list: Option<(&'a Candidates, String)>,
    /// The cells of a region in the answer.
    region: Vec<CellRect>,
}

impl<'a> Parts<'a> {
    /// The innermost prompt's parts, if one is open.
    pub(super) fn of(editor: &'a Editor) -> Option<Self> {
        let prompt = editor.prompt()?;
        let layout = editor.prompt_layout()?;
        let mask = (prompt.kind() == Kind::Passphrase).then(|| {
            let (window, buffer) = editor.current();
            let cursor = window.selections.primary().cursor;
            let (stars, column) = minibuffer::mask(buffer.text(), cursor, layout.input.columns);
            ("*".repeat(stars), column)
        });
        let list = prompt.candidates().map(|candidates| {
            let place = candidates.selected().map_or(0, |place| place + 1);
            (candidates, format!("{place}/{}", candidates.len()))
        });
        let mut region = Vec::new();
        editor.prompt_region_cells(&mut region);
        Some(Self {
            layout,
            label: prompt.label(),
            kind: prompt.kind(),
            message: prompt.message(),
            mask,
            list,
            region,
        })
    }

    /// The spans of the candidate rows shown, and of the count, right-aligned in its cells.
    fn list_spans(&self) -> (Vec<Vec<Span<'_>>>, [Span<'_>; 2]) {
        let Some((candidates, count)) = &self.list else {
            return (Vec::new(), [blank(), blank()]);
        };
        let pad = self.layout.count.columns.saturating_sub(count.len());
        let count = [
            Span {
                text: spaces(pad),
                face: Face::Annotation,
            },
            Span {
                text: count,
                face: Face::Annotation,
            },
        ];
        let shown = self.layout.regions.candidates;
        let mut highlights = Vec::new();
        let rows = (candidates.top()..candidates.len())
            .take(shown.rows)
            .filter_map(|place| candidates.get(place))
            .map(|candidate| {
                candidates.query().highlights(candidate, &mut highlights);
                candidate_spans(candidate, &highlights, shown.columns)
            })
            .collect();
        (rows, count)
    }

    /// The spans of its rows.
    pub(super) fn spans(&self) -> Spans<'_> {
        // A question's label reads as the text does, and a prompt's is dim beside its answer.
        let face = if self.kind.is_question() {
            Face::Float
        } else {
            Face::Prompt
        };
        let rows = &self.layout.label;
        let label = rows
            .iter()
            .enumerate()
            .map(|(index, range)| {
                let cut = if index + 1 == rows.len() && self.layout.label_cut {
                    minibuffer::CUT
                } else {
                    ""
                };
                [&self.label[range.clone()], cut].map(|text| Span { text, face })
            })
            .collect();
        let (said, cut) =
            minibuffer::fit_message(self.message.unwrap_or(""), self.layout.message.columns);
        let message = [" [", said, cut, "]"].map(|text| Span {
            text,
            face: Face::Float,
        });
        let (candidates, count) = self.list_spans();
        Spans {
            label,
            message,
            keys: self.keys(),
            candidates,
            count,
        }
    }

    /// A question's keys and their verbs, each at the column `minibuffer::layout` gives it,
    /// from the first key's: spaces up to it, then what it shows. Empty for a prompt.
    fn keys(&self) -> [Span<'a>; 12] {
        let mut spans = std::array::from_fn(|_| Span {
            text: "",
            face: Face::Float,
        });
        let Kind::Question { verbs, .. } = self.kind else {
            return spans;
        };
        let [y, n] = self.layout.keys;
        let [yes, no] = self.layout.verbs;
        let mut at = y.column;
        let parts = [
            (y, ["", minibuffer::KEYS[0]], Face::Key),
            (yes, [" ", verbs[0]], Face::Prompt),
            (n, ["", minibuffer::KEYS[1]], Face::Key),
            (no, [" ", verbs[1]], Face::Prompt),
        ];
        for (index, (cells, texts, face)) in parts.into_iter().enumerate() {
            if cells.columns == 0 {
                continue;
            }
            let gap = cells.column.saturating_sub(at).min(SPACES.len());
            spans[index * 3] = Span {
                text: &SPACES[..gap],
                face: Face::Float,
            };
            spans[index * 3 + 1] = Span {
                text: texts[0],
                face,
            };
            spans[index * 3 + 2] = Span {
                text: texts[1],
                face,
            };
            at = cells.column + cells.columns;
        }
        spans
    }
}

/// Spaces for the gaps between a question's keys and in a candidate's row.
const SPACES: &str = concat!(
    "                                                                ",
    "                                                                ",
    "                                                                ",
    "                                                                ",
);

/// `n` spaces, at most as many as [`SPACES`] holds.
fn spaces(n: usize) -> &'static str {
    &SPACES[..n.min(SPACES.len())]
}

fn blank<'a>() -> Span<'a> {
    Span {
        text: "",
        face: Face::Float,
    }
}

/// The spans of a candidate's row of `columns` cells: its text with what matched in
/// [`Face::Match`], its key and its annotation, as `minibuffer::candidate_row` places them.
fn candidate_spans<'a>(
    candidate: &'a oso_core::complete::Candidate,
    highlights: &[Range<usize>],
    columns: usize,
) -> Vec<Span<'a>> {
    let text = candidate.text();
    let row = minibuffer::candidate_row(text, candidate.key(), candidate.annotation(), columns);
    let shown = &text[..row.text];
    let mut spans = Vec::with_capacity(2 * highlights.len() + 6);
    let mut at = 0;
    for range in highlights {
        let (start, end) = (range.start.min(shown.len()), range.end.min(shown.len()));
        if start >= end {
            continue;
        }
        spans.push(Span {
            text: &shown[at..start],
            face: Face::Float,
        });
        spans.push(Span {
            text: &shown[start..end],
            face: Face::Match,
        });
        at = end;
    }
    spans.push(Span {
        text: &shown[at..],
        face: Face::Float,
    });
    let mut used = cells_of(shown);
    if row.text_cut {
        spans.push(Span {
            text: minibuffer::CUT,
            face: Face::Float,
        });
        used += 1;
    }
    if let Some(column) = row.key {
        spans.push(Span {
            text: spaces(column.saturating_sub(used)),
            face: Face::Float,
        });
        spans.push(Span {
            text: candidate.key(),
            face: Face::Binding,
        });
        used = column + cells_of(candidate.key());
    }
    if let Some((column, bytes, cut)) = row.annotation {
        spans.push(Span {
            text: spaces(column.saturating_sub(used)),
            face: Face::Float,
        });
        spans.push(Span {
            text: &candidate.annotation()[..bytes],
            face: Face::Annotation,
        });
        if cut {
            spans.push(Span {
                text: minibuffer::CUT,
                face: Face::Annotation,
            });
        }
    }
    spans
}

/// The spans of a float's rows, which its lines borrow.
pub(super) struct Spans<'a> {
    label: Vec<[Span<'a>; 2]>,
    message: [Span<'a>; 4],
    keys: [Span<'a>; 12],
    candidates: Vec<Vec<Span<'a>>>,
    count: [Span<'a>; 2],
}

impl<'a> Spans<'a> {
    /// The rows as the panes take them.
    pub(super) fn lines(&'a self) -> Lines<'a> {
        Lines {
            label: self.label.iter().map(|row| &row[..]).collect(),
            message: [&self.message],
            keys: [&self.keys],
            candidates: self.candidates.iter().map(|row| &row[..]).collect(),
            count: [&self.count],
        }
    }
}

/// A float's rows, as [`PaneContent::Lines`] takes them.
pub(super) struct Lines<'a> {
    label: Vec<&'a [Span<'a>]>,
    message: [&'a [Span<'a>]; 1],
    keys: [&'a [Span<'a>]; 1],
    candidates: Vec<&'a [Span<'a>]>,
    count: [&'a [Span<'a>]; 1],
}

/// Adds the float's panes to `list`, after the tiled ones.
pub(super) fn panes<'a>(
    editor: &'a Editor,
    parts: &'a Parts<'a>,
    lines: &'a Lines<'a>,
    list: &mut Vec<Pane<'a>>,
) {
    let layout = &parts.layout;
    let regions = layout.regions;
    let inner = regions.inner;
    // A frame of no rows has no room for a float.
    if inner.rows == 0 {
        return;
    }
    // The answer's row, or the keys'.
    let last = inner.row + inner.rows - 1;
    // A pane's glyphs keep a band only into the float's padding, where its region borders on
    // it, and each row of padding goes to the panes of one row at most, so that no glyph draws
    // over another pane of the float or outside it (ADR 0072, ADR 0075). The gap and the
    // padding below both go to the answer's row; the label keeps to its own rows.
    let content = |id, region: CellRect, content| Pane {
        id: PaneId(id),
        region,
        layer: Layer::FLOAT_CONTENT,
        background: None,
        panel: None,
        bands: Bands {
            above: regions.gap && region.row == last,
            below: regions.padded_below() && region.row == last,
        },
        content,
    };
    let field = Some(layout.field).filter(|field| field.columns > 0);
    let keys: &[CellRect] = if parts.kind.is_question() {
        &layout.keys
    } else {
        &[]
    };
    // The panel's pane is the text area beneath, which its shadow stays in (ADR 0075).
    list.push(Pane {
        id: PaneId(3),
        region: editor.frame().window.text,
        layer: Layer::FLOAT,
        background: None,
        panel: Some(Panel {
            cells: regions.outer,
            field,
            field_room: layout.field_room,
            keys,
            selected: selected_row(parts),
        }),
        bands: Bands::NONE,
        content: PaneContent::Lines(&[]),
    });
    if layout.label_cells.rows > 0 && layout.label_cells.columns > 0 {
        list.push(content(
            4,
            layout.label_cells,
            PaneContent::Lines(&lines.label),
        ));
    }
    let [y, _] = layout.keys;
    if parts.kind.is_question() && y.columns > 0 {
        let end = inner.column + inner.columns;
        let region = CellRect {
            columns: end - y.column,
            ..y
        };
        list.push(content(5, region, PaneContent::Lines(&lines.keys)));
    }
    if layout.input.columns > 0 {
        list.push(content(6, layout.input, answer(editor, parts)));
    }
    if parts.message.is_some() && layout.message.columns > 0 {
        list.push(content(
            7,
            layout.message,
            PaneContent::Lines(&lines.message),
        ));
    }
    list_panes(parts, lines, list);
}

/// Adds a list's panes to `list`: its count, and its candidates' rows, which keep the padding
/// after them and none of the answer's above (ADR 0076).
fn list_panes<'a>(parts: &'a Parts<'a>, lines: &'a Lines<'a>, list: &mut Vec<Pane<'a>>) {
    let layout = &parts.layout;
    let regions = layout.regions;
    let count = Pane {
        id: PaneId(8),
        region: layout.count,
        layer: Layer::FLOAT_CONTENT,
        background: None,
        panel: None,
        bands: Bands::NONE,
        content: PaneContent::Lines(&lines.count),
    };
    if parts.list.is_some() && layout.count.columns > 0 {
        list.push(count);
    }
    let candidates = regions.candidates;
    if parts.list.is_some() && candidates.rows > 0 && candidates.columns > 0 {
        list.push(Pane {
            id: PaneId(9),
            region: candidates,
            layer: Layer::FLOAT_CONTENT,
            background: None,
            panel: None,
            bands: Bands {
                above: false,
                below: regions.candidates_padded(),
            },
            content: PaneContent::Lines(&lines.candidates),
        });
    }
}

/// The cells of the selected candidate's row, while it shows.
fn selected_row(parts: &Parts<'_>) -> Option<CellRect> {
    let (candidates, _) = parts.list.as_ref()?;
    let rows = parts.layout.regions.candidates;
    let place = candidates.selected()?.checked_sub(candidates.top())?;
    (place < rows.rows).then_some(CellRect {
        row: rows.row + place,
        rows: 1,
        ..rows
    })
}

/// What the answer's pane shows: the prompt's window, or one `*` per character of a
/// passphrase.
fn answer<'a>(editor: &'a Editor, parts: &'a Parts<'a>) -> PaneContent<'a> {
    if let Some((text, cursor)) = &parts.mask {
        return PaneContent::Masked(Masked {
            text,
            cursor: *cursor,
        });
    }
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let cursor = window.cursor_point(&display);
    PaneContent::Text(TextView {
        display,
        buffer: window.buffer(),
        revision: buffer.revision(),
        scroll: window.scroll(),
        cursor,
        cursor_style: CursorStyle::Box,
        preedit: editor.preedit(),
        region_cells: &parts.region,
    })
}
