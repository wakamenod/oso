//! The font styles and underlines a row's text takes from its highlights, where its view has any
//! (ADR 0165, "The look: styles").
//!
//! Must not depend on: the GPU, `text` or the display model.
//!
//! - They are put on the row once it is read, so a style is what the shaper is given, and
//!   compared as the text is. While the revision holds, the row's runs are walked again and
//!   compared with those it was read with: highlights that come late shape it again, and nothing
//!   stale is drawn. That walk is a seek a row, as its colours take, and its cursor's allocation.
//! - A style starts and ends at a cluster's first byte, so none is cut.
//! - A row of text in no view of styles costs one test that it had none.

use oso_core::syntax::Highlights;
use oso_platform::text::FontStyle;

use super::{Rows, ShapedRow};
use crate::pane::{Face, Look};

/// The most stretches of a row's text in a style of their own, each a piece to shape, and the
/// most underlined, each a rectangle: past them, a row's text is regular, and has no line under
/// it. A row of text has a few; a crafted one could have one every other column.
pub(super) const MAX_STRETCHES: usize = 8;

/// What a row draws besides its text and colours.
#[derive(Default)]
pub(super) struct RowLooks {
    /// Where in the buffer the text's look changes, from none, and what to: what [`same`]
    /// compares while the revision holds.
    changes: Vec<(usize, Look)>,
    /// Where in the row's text the font style changes, from regular, and what to.
    pub(super) styles: Vec<(u32, FontStyle)>,
    /// Underlined cells: their first column, from the row's base, and how many.
    underlines: Vec<(usize, usize)>,
}

impl RowLooks {
    pub(super) fn clear(&mut self) {
        self.changes.clear();
        self.styles.clear();
        self.underlines.clear();
    }
}

impl ShapedRow {
    /// Where its font style changes.
    pub(super) fn styles(&self) -> &[(u32, FontStyle)] {
        self.looks.as_ref().map_or(&[], |looks| &looks.styles)
    }

    /// Its underlined cells, from its base.
    pub(crate) fn underlines(&self) -> &[(usize, usize)] {
        self.looks.as_ref().map_or(&[], |looks| &looks.underlines)
    }

    /// Takes `looks`, as it was read with them.
    pub(super) fn keep_looks(&mut self, looks: &RowLooks) {
        // No change means no style and no underline.
        if looks.changes.is_empty() {
            self.looks = None;
        } else {
            let kept = self.looks.get_or_insert_default();
            kept.changes.clone_from(&looks.changes);
            kept.styles.clone_from(&looks.styles);
            kept.underlines.clone_from(&looks.underlines);
        }
    }
}

impl Rows {
    /// Puts the styles and the underlines `highlights` gives on the row just read, cutting its
    /// pieces where a style changes. An escape, a box, a whitespace mark and the wrap mark stay
    /// regular.
    pub(super) fn put_looks(&mut self, highlights: &Highlights) {
        self.looks
            .changes
            .extend(changes(highlights, &self.clusters));
        let mut stretches = Stretches::default();
        let (mut last, read) = (FontStyle::Regular, self.breaks.len());
        let mut clusters = self.clusters.iter().peekable();
        let mut changes = self.looks.changes.iter().peekable();
        let mut look = Look::default();
        for grapheme in &self.graphemes {
            while let Some((_, offset)) = clusters.next_if(|(start, _)| *start <= grapheme.column) {
                while let Some((_, next)) = changes.next_if(|(start, _)| start <= offset) {
                    look = *next;
                }
            }
            if look.underlined && grapheme.face != Face::WrapMark {
                let cells = grapheme.cells as usize;
                push_underline(&mut self.looks.underlines, grapheme.column, cells);
            }
            let style = if grapheme.face == Face::Text {
                stretches.next(look.style)
            } else {
                FontStyle::Regular
            };
            if style != last {
                last = style;
                self.looks.styles.push((grapheme.byte, style));
                self.breaks.push(grapheme.byte);
            }
        }
        if self.breaks.len() > read {
            self.breaks.sort_unstable();
            self.breaks.dedup();
        }
    }
}

/// Where the look of a row of `clusters` changes, from none, and what to: a cluster takes the one
/// its first byte has. A seek, and a step a run.
fn changes(
    highlights: &Highlights,
    clusters: &[(usize, usize)],
) -> impl Iterator<Item = (usize, Look)> {
    let range = match (clusters.first(), clusters.last()) {
        (Some(&(_, first)), Some(&(_, last))) => first..last.saturating_add(1),
        _ => 0..0,
    };
    let mut last = Look::default();
    (highlights.runs(range)).filter_map(move |(range, highlight)| {
        let look = Look::of(highlight);
        (std::mem::replace(&mut last, look) != look).then_some((range.start, look))
    })
}

/// Whether a row of `clusters`, read with `looks`, has the same from `styles` now, at the same
/// revision of its buffer. Nothing without `styles`.
pub(super) fn same(
    styles: Option<&Highlights>,
    clusters: &[(usize, usize)],
    looks: Option<&RowLooks>,
) -> bool {
    let was = looks.map_or(&[][..], |looks| &looks.changes);
    match styles {
        Some(highlights) if !clusters.is_empty() => {
            changes(highlights, clusters).eq(was.iter().copied())
        }
        _ => was.is_empty(),
    }
}

/// The stretches of a style of a row's clusters, in order, up to [`MAX_STRETCHES`] of them.
#[derive(Default)]
struct Stretches {
    last: FontStyle,
    count: usize,
}

impl Stretches {
    /// The style the next cluster is shaped in, if it asks for `style`.
    fn next(&mut self, style: FontStyle) -> FontStyle {
        let mut style = style;
        if style != self.last && style != FontStyle::Regular {
            if self.count == MAX_STRETCHES {
                style = FontStyle::Regular;
            } else {
                self.count += 1;
            }
        }
        self.last = style;
        style
    }
}

/// Adds `cells` at `column` to `underlines`, joining the last if it ends there, up to
/// [`MAX_STRETCHES`] of them.
fn push_underline(underlines: &mut Vec<(usize, usize)>, column: usize, cells: usize) {
    let room = underlines.len() < MAX_STRETCHES;
    match underlines.last_mut() {
        Some((start, width)) if *start + *width == column => *width += cells,
        _ if room => underlines.push((column, cells)),
        _ => {}
    }
}

#[cfg(test)]
mod tests;
