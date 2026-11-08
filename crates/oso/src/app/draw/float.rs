//! The panes of an open prompt's float: its background and border, what it says, the
//! answer and a message (ADR 0072).
//!
//! Must not depend on: key presses or commands; a frame only reads the editor's state.
//!
//! The float's background and border are one pane in [`Layer::FLOAT`], over the tiled
//! panes, and what it shows is in [`Layer::FLOAT_CONTENT`], in panes that never overlap
//! one another. A passphrase shows one `*` per character, and nothing of it reaches a
//! pane.

use oso_core::editor::Editor;
use oso_core::frame::CellRect;
use oso_core::minibuffer::{self, Kind, PromptLayout};
use oso_render::{CursorStyle, Face, Layer, Masked, Pane, PaneContent, PaneId, Span, TextView};

/// The most panes a float takes.
pub(super) const MAX_PANES: usize = 5;

/// What the innermost prompt shows, worked out for one frame.
pub(super) struct Parts<'a> {
    layout: &'a PromptLayout,
    label: &'a str,
    kind: Kind,
    message: Option<&'a str>,
    /// For a passphrase: one `*` per character in view, and the cursor's column among them.
    mask: Option<(String, usize)>,
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
        Some(Self {
            layout,
            label: prompt.label(),
            kind: prompt.kind(),
            message: prompt.message(),
            mask,
        })
    }

    /// The spans of its rows.
    pub(super) fn spans(&self) -> Spans<'_> {
        let rows = &self.layout.label;
        let label = rows
            .iter()
            .enumerate()
            .map(|(index, range)| {
                let last = index + 1 == rows.len();
                let cut = if last && self.layout.label_cut {
                    minibuffer::CUT
                } else {
                    ""
                };
                [
                    Span {
                        text: &self.label[range.clone()],
                        face: Face::Prompt,
                    },
                    Span {
                        text: cut,
                        face: Face::Prompt,
                    },
                ]
            })
            .collect();
        let (said, cut) =
            minibuffer::fit_message(self.message.unwrap_or(""), self.layout.message.columns);
        let message = [" [", said, cut, "]"].map(|text| Span {
            text,
            face: Face::Float,
        });
        Spans { label, message }
    }
}

/// The spans of a float's rows, which its lines borrow.
pub(super) struct Spans<'a> {
    label: Vec<[Span<'a>; 2]>,
    message: [Span<'a>; 4],
}

impl<'a> Spans<'a> {
    /// The rows as the panes take them.
    pub(super) fn lines(&'a self) -> Lines<'a> {
        let mut label: Vec<&[Span<'a>]> = self.label.iter().map(|row| &row[..]).collect();
        let last = label.pop().map_or([&[][..]], |row| [row]);
        Lines {
            label,
            last,
            message: [&self.message],
        }
    }
}

/// A float's rows, as [`PaneContent::Lines`] takes them.
pub(super) struct Lines<'a> {
    /// The label's rows but the last.
    label: Vec<&'a [Span<'a>]>,
    /// The label's last row, which the answer follows.
    last: [&'a [Span<'a>]; 1],
    message: [&'a [Span<'a>]; 1],
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
    let content = |id, region, content| Pane {
        id: PaneId(id),
        region,
        layer: Layer::FLOAT_CONTENT,
        background: None,
        border: None,
        content,
    };
    list.push(Pane {
        id: PaneId(3),
        region: regions.outer,
        layer: Layer::FLOAT,
        background: Some(Face::Float),
        border: regions.framed().then_some(Face::FloatBorder),
        content: PaneContent::Lines(&[]),
    });
    let above = lines.label.len();
    if above > 0 {
        let region = CellRect {
            rows: above.min(inner.rows),
            ..inner
        };
        list.push(content(4, region, PaneContent::Lines(&lines.label)));
    }
    let row = layout.input.row;
    let end = if parts.kind.is_question() {
        layout.message.column.min(inner.column + inner.columns)
    } else {
        layout.input.column
    };
    let last = CellRect {
        row,
        column: inner.column,
        rows: usize::from(inner.rows > above),
        columns: end - inner.column,
    };
    list.push(content(5, last, PaneContent::Lines(&lines.last)));
    if layout.input.columns > 0 {
        let input = match &parts.mask {
            Some((text, cursor)) => PaneContent::Masked(Masked {
                text,
                cursor: *cursor,
            }),
            None => {
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
                })
            }
        };
        list.push(content(6, layout.input, input));
    }
    if parts.message.is_some() && layout.message.columns > 0 {
        list.push(content(
            7,
            layout.message,
            PaneContent::Lines(&lines.message),
        ));
    }
}
