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

use oso_core::complete::{self, Candidates};
use oso_core::editor::{Editor, Preview};
use oso_core::frame::CellRect;
use oso_core::minibuffer::{self, Kind, PromptLayout, cells_of};
use oso_core::window::Scroll;
use oso_render::{
    Bands, CursorStyle, Face, Layer, Masked, Pane, PaneContent, PaneId, Panel, Span, TextView,
};

/// The most panes a float takes.
pub(super) const MAX_PANES: usize = 9;

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
            // While a listing or a worker's result is on its way, the rows shown stay and the
            // count says so (ADR 0076, ADR 0087, ADR 0096).
            let count = if editor.files_pending() || candidates.pending() {
                "…/…".to_owned()
            } else {
                {
                    // Past a limit, `+` says some were left out (ADR 0087).
                    let more = if editor.files_cut() { "+" } else { "" };
                    format!("{place}/{}{more}", candidates.len())
                }
            };
            (candidates, count)
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

    /// The spans of the candidate rows shown, their annotations in one column, and of the
    /// count.
    fn list_spans(&self) -> (Vec<Vec<Span<'_>>>, [Span<'_>; 2]) {
        let Some((candidates, count)) = &self.list else {
            return (Vec::new(), [blank(), blank()]);
        };
        let count = [
            Span {
                text: count,
                face: Face::CompletionPrompt,
            },
            blank(),
        ];
        let shown = self.layout.regions.candidates;
        let named = candidates
            .shown(shown.rows)
            .map(|(_, candidate)| (candidate.text(), candidate.binding()));
        let column = minibuffer::annotation_column(named, shown.columns);
        let mut highlights = Vec::new();
        let rows = candidates
            .shown(shown.rows)
            .map(|(_, candidate)| {
                candidates.query().highlights(candidate, &mut highlights);
                candidate_spans(candidate, &highlights, shown.columns, column)
            })
            .collect();
        (rows, count)
    }

    /// The spans of its rows.
    pub(super) fn spans(&self) -> Spans<'_> {
        // A question's label is in a colour no buffer text is drawn in (ADR 0089), a prompt's is
        // dim beside its answer, and a completion's takes its count's accent.
        let face = if self.kind.is_question() {
            Face::Question
        } else if self.list.is_some() {
            Face::CompletionPrompt
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
        let divider = [Span {
            text: DIVIDER,
            face: Face::CompletionAnnotation,
        }];
        let withheld = [Span {
            text: WITHHELD,
            face: Face::CompletionAnnotation,
        }];
        Spans {
            label,
            message,
            keys: self.keys(),
            candidates,
            count,
            divider,
            withheld,
            region: &self.region,
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

/// Pushes `n` spaces onto `spans`, in pieces of [`SPACES`], so that no gap is cut short however
/// wide the row.
fn push_spaces(spans: &mut Vec<Span<'_>>, mut n: usize) {
    while n > 0 {
        let text = spaces(n);
        spans.push(Span {
            text,
            face: Face::Float,
        });
        n -= text.len();
    }
}

fn blank<'a>() -> Span<'a> {
    Span {
        text: "",
        face: Face::Float,
    }
}

/// The spans of a candidate's row of `columns` cells: its text with what matched in
/// [`Face::CompletionMatch`], its binding in parentheses, and its annotation at `column`, as
/// `minibuffer::candidate_row` places them.
fn candidate_spans<'a>(
    candidate: &'a oso_core::complete::Candidate,
    highlights: &[Range<usize>],
    columns: usize,
    column: usize,
) -> Vec<Span<'a>> {
    let text = candidate.text();
    let (binding, annotation) = (candidate.binding(), candidate.annotation());
    let row = minibuffer::candidate_row(text, binding, annotation, columns, column);
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
            face: Face::CompletionMatch,
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
    if let Some(column) = row.binding {
        push_spaces(&mut spans, column.saturating_sub(used));
        for text in [minibuffer::BINDING_OPEN, binding, minibuffer::BINDING_CLOSE] {
            spans.push(Span {
                text,
                face: Face::CompletionBinding,
            });
        }
        used = column + cells_of(binding) + 2;
    }
    if let Some((column, bytes, cut)) = row.annotation {
        push_spaces(&mut spans, column.saturating_sub(used));
        spans.push(Span {
            text: &candidate.annotation()[..bytes],
            face: Face::CompletionAnnotation,
        });
        if cut {
            spans.push(Span {
                text: minibuffer::CUT,
                face: Face::CompletionAnnotation,
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
    /// The column between the candidates and a preview.
    divider: [Span<'a>; 1],
    /// What a preview says in place of a buffer that may hold secrets.
    withheld: [Span<'a>; 1],
    /// The cells of a region in the answer.
    region: &'a [CellRect],
}

/// The column between the candidates and a preview, one cell a row.
const DIVIDER: &str = "│";

/// What a preview shows of a buffer that may hold secrets, or one not judged yet (ADR 0088).
const WITHHELD: &str = "may hold secrets";

impl<'a> Spans<'a> {
    /// The rows as the panes take them.
    pub(super) fn lines(&'a self) -> Lines<'a> {
        Lines {
            label: self.label.iter().map(|row| &row[..]).collect(),
            message: [&self.message],
            keys: [&self.keys],
            candidates: self.candidates.iter().map(|row| &row[..]).collect(),
            count: [&self.count],
            divider: vec![&self.divider[..]; complete::ROWS],
            withheld: [&self.withheld],
            fills: [(Face::Region, self.region)],
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
    /// The divider's rows, one for each candidate row there can be.
    divider: Vec<&'a [Span<'a>]>,
    withheld: [&'a [Span<'a>]; 1],
    /// What the answer's pane fills behind its text: its region (ADR 0076).
    fills: [(Face, &'a [CellRect]); 1],
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
            // A completion's float sits at the center, framed by a border (ADR 0076).
            border: parts.list.is_some(),
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
        list.push(content(6, layout.input, answer(editor, parts, lines)));
    }
    if parts.message.is_some() && layout.message.columns > 0 {
        list.push(content(
            7,
            layout.message,
            PaneContent::Lines(&lines.message),
        ));
    }
    list_panes(parts, lines, list);
    preview_panes(editor, parts, lines, list);
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

/// The base of a preview pane's id: each previewed buffer's pane has its own, so that the rows
/// shaped for it are kept while that buffer stays selected (ADR 0088).
const PREVIEW_PANES: u32 = 1 << 20;

/// Adds a preview's panes to `list`: the column that divides it from the candidates, and the
/// selected buffer from where its window was, read-only with its cursor hollow, or what is
/// said of a buffer that may hold secrets in its place (ADR 0088).
fn preview_panes<'a>(
    editor: &'a Editor,
    parts: &'a Parts<'a>,
    lines: &'a Lines<'a>,
    list: &mut Vec<Pane<'a>>,
) {
    let regions = parts.layout.regions;
    let preview = regions.preview;
    if parts.list.is_none() || preview.rows == 0 || preview.columns == 0 {
        return;
    }
    let pane = |id, region, content| Pane {
        id: PaneId(id),
        region,
        layer: Layer::FLOAT_CONTENT,
        background: None,
        panel: None,
        bands: Bands::NONE,
        content,
    };
    let divider = CellRect {
        column: preview.column - 1,
        columns: 1,
        ..preview
    };
    let rows = &lines.divider[..preview.rows.min(lines.divider.len())];
    list.push(pane(10, divider, PaneContent::Lines(rows)));
    let content = match editor.preview() {
        None => return,
        Some(Preview::Withheld) => pane(11, preview, PaneContent::Lines(&lines.withheld)),
        Some(Preview::Buffer { id, top, cursor }) => {
            let Some(buffer) = editor.buffer(id) else {
                return;
            };
            let display = buffer.display();
            let cursor = display.offset_to_display_point(cursor);
            let view = TextView {
                display,
                buffer: id,
                revision: buffer.revision(),
                scroll: Scroll { top, left: 0 },
                cursor,
                cursor_style: CursorStyle::Hollow,
                preedit: None,
                fills: &[],
            };
            let number = u32::try_from(id.get()).unwrap_or(u32::MAX);
            pane(
                PREVIEW_PANES.saturating_add(number),
                preview,
                PaneContent::Text(view),
            )
        }
    };
    list.push(content);
}

/// The cells of the selected candidate's row, while it shows, or of the answer's row while what
/// is typed is selected (ADR 0087, ADR 0088).
fn selected_row(parts: &Parts<'_>) -> Option<CellRect> {
    let (candidates, _) = parts.list.as_ref()?;
    let rows = parts.layout.regions.candidates;
    if candidates.on_input() {
        // Across the whole answer's row, also beside a preview.
        let inner = parts.layout.regions.inner;
        return Some(CellRect {
            row: (inner.row + inner.rows).checked_sub(1)?,
            rows: 1,
            ..inner
        });
    }
    let place = candidates.selected()?.checked_sub(candidates.top())?;
    (place < rows.rows).then_some(CellRect {
        row: rows.row + place,
        rows: 1,
        ..rows
    })
}

/// What the answer's pane shows: the prompt's window, or one `*` per character of a
/// passphrase.
fn answer<'a>(editor: &'a Editor, parts: &'a Parts<'a>, lines: &'a Lines<'a>) -> PaneContent<'a> {
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
        fills: &lines.fills,
    })
}
