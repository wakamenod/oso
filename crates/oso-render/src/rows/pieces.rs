//! The shaping of a row's pieces: each shaped on its own, and the short ones kept by their
//! text and style, so that a label or a name is shaped once (ADR 0068).
//!
//! Must not depend on: the GPU or `text`.

use std::collections::HashMap;

use oso_platform::text::{FontStyle, ShapedGlyph, TextSystem};

/// The longest piece of a row whose shaping is kept by its text, in bytes, and
/// how many are kept. A shaper's output depends only on its text, its style and the font,
/// so a short piece, such as a label, a cut cluster's character or a name on
/// the modeline, is shaped once and not again for each row or frame it is in.
pub(super) const KEPT_PIECE_BYTES: usize = 64;
const KEPT_PIECES: usize = 1024;

/// Shaped pieces of a row, by their style and text, for pieces of up to
/// [`KEPT_PIECE_BYTES`]. They hold buffer text, so they go with the rows they
/// came from and at each revision (ADR 0028). No `Debug` for the same reason.
#[derive(Default)]
pub(super) struct Pieces {
    /// Where each piece's glyphs are in `shaped`, by style.
    index: [HashMap<Box<str>, usize>; STYLES],
    shaped: Vec<Vec<ShapedGlyph>>,
    /// Scratch space for a piece that is too long to keep.
    long: Vec<ShapedGlyph>,
}

/// The font styles a piece can be in, and the place of each.
const STYLES: usize = 3;

fn slot(style: FontStyle) -> usize {
    match style {
        FontStyle::Regular => 0,
        FontStyle::Bold => 1,
        FontStyle::Italic => 2,
    }
}

impl Pieces {
    /// The glyphs of `text` in `style`, shaped alone.
    fn shape(
        &mut self,
        system: &mut dyn TextSystem,
        text: &str,
        style: FontStyle,
    ) -> &[ShapedGlyph] {
        if text.len() > KEPT_PIECE_BYTES {
            system.shape(text, style, &mut self.long);
            return &self.long;
        }
        let index = slot(style);
        if let Some(&at) = self.index[index].get(text) {
            return &self.shaped[at];
        }
        if self.shaped.len() >= KEPT_PIECES {
            self.clear();
        }
        let mut glyphs = Vec::new();
        system.shape(text, style, &mut glyphs);
        self.index[index].insert(text.into(), self.shaped.len());
        self.shaped.push(glyphs);
        &self.shaped[self.shaped.len() - 1]
    }

    pub(super) fn clear(&mut self) {
        self.index.iter_mut().for_each(HashMap::clear);
        self.shaped.clear();
    }
}

/// Shapes `text` into `out`. Each piece from a start in `breaks` is shaped on its
/// own, through `pieces`, and with no breaks the text is one piece. `styles` says where the font
/// style changes, from regular, always at a piece's start.
pub(super) fn shape_pieces(
    system: &mut dyn TextSystem,
    text: &str,
    (breaks, styles): (&[u32], &[(u32, FontStyle)]),
    pieces: &mut Pieces,
    out: &mut Vec<ShapedGlyph>,
) {
    if breaks.is_empty() {
        let style = styles
            .first()
            .map_or(FontStyle::Regular, |(_, style)| *style);
        system.shape(text, style, out);
        return;
    }
    out.clear();
    let mut pen = 0.0;
    let mut styles = styles.iter().peekable();
    let mut style = FontStyle::Regular;
    for (index, &start) in breaks.iter().enumerate() {
        let end = breaks
            .get(index + 1)
            .map_or(text.len(), |&end| end as usize);
        while let Some((_, next)) = styles.next_if(|(byte, _)| *byte <= start) {
            style = *next;
        }
        let piece = pieces.shape(system, &text[start as usize..end], style);
        for glyph in piece {
            out.push(ShapedGlyph {
                cluster: glyph.cluster + start,
                x: glyph.x + pen,
                ..*glyph
            });
        }
        pen += piece.iter().map(|glyph| glyph.advance).sum::<f32>();
    }
}
