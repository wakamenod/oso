//! The shaping of a row's pieces: each shaped on its own, and the short ones kept by their
//! text, so that a label or a name is shaped once (ADR 0068).
//!
//! Must not depend on: the GPU or `text`.

use std::collections::HashMap;

use oso_platform::text::{ShapedGlyph, TextSystem};

/// The longest piece of a row whose shaping is kept by its text, in bytes, and
/// how many are kept. A shaper's output depends only on its text and the font,
/// so a short piece, such as a label, a cut cluster's character or a name on
/// the modeline, is shaped once and not again for each row or frame it is in.
pub(super) const KEPT_PIECE_BYTES: usize = 64;
const KEPT_PIECES: usize = 1024;

/// Shaped pieces of a row, by their text, for pieces of up to
/// [`KEPT_PIECE_BYTES`]. They hold buffer text, so they go with the rows they
/// came from and at each revision (ADR 0028). No `Debug` for the same reason.
#[derive(Default)]
pub(super) struct Pieces {
    /// Where each piece's glyphs are in `shaped`.
    index: HashMap<Box<str>, usize>,
    shaped: Vec<Vec<ShapedGlyph>>,
    /// Scratch space for a piece that is too long to keep.
    long: Vec<ShapedGlyph>,
}

impl Pieces {
    /// The glyphs of `text` shaped alone.
    fn shape(&mut self, system: &mut dyn TextSystem, text: &str) -> &[ShapedGlyph] {
        if text.len() > KEPT_PIECE_BYTES {
            system.shape(text, &mut self.long);
            return &self.long;
        }
        if let Some(&at) = self.index.get(text) {
            return &self.shaped[at];
        }
        if self.shaped.len() >= KEPT_PIECES {
            self.clear();
        }
        let mut glyphs = Vec::new();
        system.shape(text, &mut glyphs);
        self.index.insert(text.into(), self.shaped.len());
        self.shaped.push(glyphs);
        &self.shaped[self.shaped.len() - 1]
    }

    pub(super) fn clear(&mut self) {
        self.index.clear();
        self.shaped.clear();
    }
}

/// Shapes `text` into `out`. Each piece from a start in `breaks` is shaped on its
/// own, through `pieces`, and with no breaks the text is one piece.
pub(super) fn shape_pieces(
    system: &mut dyn TextSystem,
    text: &str,
    breaks: &[u32],
    pieces: &mut Pieces,
    out: &mut Vec<ShapedGlyph>,
) {
    if breaks.is_empty() {
        system.shape(text, out);
        return;
    }
    out.clear();
    let mut pen = 0.0;
    for (index, &start) in breaks.iter().enumerate() {
        let end = breaks
            .get(index + 1)
            .map_or(text.len(), |&end| end as usize);
        let piece = pieces.shape(system, &text[start as usize..end]);
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
