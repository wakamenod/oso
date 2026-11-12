//! The spans of a row of a float's list: a candidate's text with what matched, its binding and
//! its annotation in their columns, a search's path and line in the annotation's face, and the
//! spaces between them (ADR 0076, ADR 0091, ADR 0114).
//!
//! Must not depend on: key presses or commands; a row only reads what it shows.

use std::ops::Range;

use oso_core::minibuffer::{self, cells_of};
use oso_render::{Face, Span};

/// What a line break in a `M-y` row reads as, drawn in the annotation's face (ADR 0091).
pub(super) const LINE_BREAK: &str = "⤶";

/// Spaces for the gaps between a question's keys and in a candidate's row.
pub(super) const SPACES: &str = concat!(
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

/// The spans of a candidate's row of `columns` cells: its text with what matched in
/// [`Face::CompletionMatch`], each [`LINE_BREAK`] in the annotation's face if `breaks` is set,
/// its binding in parentheses, and its annotation at `column`, as `minibuffer::candidate_row`
/// places them.
pub(super) fn candidate_spans<'a>(
    candidate: &'a oso_core::complete::Candidate,
    (prefix, highlights): (usize, &[Range<usize>]),
    (columns, column): (usize, usize),
    breaks: bool,
) -> Vec<Span<'a>> {
    let text = candidate.text();
    let (binding, annotation) = (candidate.binding(), candidate.annotation());
    let row = minibuffer::candidate_row(text, binding, annotation, columns, column);
    let shown = &text[..row.text];
    let mut spans = Vec::with_capacity(2 * highlights.len() + 7);
    // The first `prefix` bytes, a search's path and line, in the annotation's face (ADR 0114).
    let mut at = prefix.min(shown.len());
    while !shown.is_char_boundary(at) {
        at -= 1;
    }
    if at > 0 {
        spans.push(Span {
            text: &shown[..at],
            face: Face::CompletionAnnotation,
        });
    }
    for range in highlights {
        let (start, end) = (range.start.min(shown.len()), range.end.min(shown.len()));
        let start = start.max(at);
        if start >= end {
            continue;
        }
        push_text(&mut spans, &shown[at..start], breaks);
        spans.push(Span {
            text: &shown[start..end],
            face: Face::CompletionMatch,
        });
        at = end;
    }
    push_text(&mut spans, &shown[at..], breaks);
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

/// Pushes `text` in [`Face::Float`], with each [`LINE_BREAK`] in it in the annotation's face
/// if `breaks` is set.
fn push_text<'a>(spans: &mut Vec<Span<'a>>, text: &'a str, breaks: bool) {
    let mut rest = text;
    while breaks && let Some(at) = rest.find(LINE_BREAK) {
        let end = at + LINE_BREAK.len();
        spans.push(Span {
            text: &rest[..at],
            face: Face::Float,
        });
        spans.push(Span {
            text: &rest[at..end],
            face: Face::CompletionAnnotation,
        });
        rest = &rest[end..];
    }
    spans.push(Span {
        text: rest,
        face: Face::Float,
    });
}
