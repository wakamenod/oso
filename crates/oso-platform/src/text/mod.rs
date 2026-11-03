//! The text system: font loading, shaping onto the cell grid and glyph
//! rasterizing, behind one trait (ADR 0005, ADR 0036).
//!
//! - macOS uses CoreText.
//! - Other systems have no backend yet: [`load`] reports
//!   [`TextError::Unsupported`] and callers fall back to [`NullTextSystem`],
//!   which draws no text (ADR 0003, Tier 2).
//!
//! All sizes are in physical pixels: fonts are loaded at the point size times
//! the window's scale factor, so glyphs are rasterized at the resolution they
//! are shown at.

#[cfg(target_os = "macos")]
mod coretext;
pub mod grid;

use std::fmt;

/// A font to load: families in order of preference, and a size in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontRequest<'a> {
    /// Family names, tried in order. The first one installed is used.
    pub families: &'a [&'a str],
    /// The size in points; one point is `scale` physical pixels.
    pub size_pt: f32,
}

/// The size of one cell of the monospace grid, in physical pixels.
///
/// `width` and `height` are whole pixels, so every cell starts on a pixel
/// boundary and glyphs stay crisp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    /// Width of a half-width cell.
    pub width: f32,
    /// Height of a row.
    pub height: f32,
    /// Distance from the top of a row down to the baseline, in whole pixels.
    pub ascent: f32,
    /// Distance from the baseline down to the bottom of a row.
    pub descent: f32,
}

impl CellMetrics {
    /// Metrics estimated from the font size alone, for when no font is
    /// loaded: a monospace cell is about 0.6 em wide and 1.2 em tall.
    pub fn estimate(size_pt: f32, scale: f64) -> Self {
        let em = size_pt * scale as f32;
        let width = (em * 0.6).round().max(1.0);
        let height = (em * 1.2).ceil().max(1.0);
        let ascent = (em * 0.95).round().min(height);
        Self {
            width,
            height,
            ascent,
            descent: height - ascent,
        }
    }
}

/// A font loaded by a text system: the primary font or a fallback font
/// chosen while shaping. Valid only for the text system that returned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontId(pub u16);

/// A glyph index within one font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphId(pub u16);

/// Identifies a rasterized glyph: what a glyph cache is keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphKey {
    pub font: FontId,
    pub glyph: GlyphId,
}

/// One glyph of a shaped run, placed on the cell grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub key: GlyphKey,
    /// The first cell of the glyph's cluster, counted from the run start.
    pub column: u32,
    /// How many cells the cluster takes: 1 for half-width, 2 for full-width.
    pub cells: u8,
    /// Byte offset of the cluster in the run's text.
    pub cluster: u32,
    /// Pen position relative to the cluster's first cell, at the baseline,
    /// in whole pixels; y grows downwards. Centers glyphs whose advance is
    /// not exactly `cells` cells, such as CJK from a fallback font.
    pub offset: [f32; 2],
}

/// How a glyph bitmap stores its pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlyphFormat {
    /// One byte of coverage per pixel, tinted with the text color.
    Coverage,
    /// Four bytes per pixel, RGBA with premultiplied alpha, drawn as is
    /// (color emoji).
    Color,
}

impl GlyphFormat {
    /// Bytes per pixel.
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Coverage => 1,
            Self::Color => 4,
        }
    }
}

/// A rasterized glyph. Rows run top to bottom with no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    /// From the pen position right to the bitmap's left edge.
    pub left: i32,
    /// From the baseline up to the bitmap's top edge.
    pub top: i32,
    pub format: GlyphFormat,
    /// `width * height * format.bytes_per_pixel()` bytes.
    pub data: Vec<u8>,
}

/// Why a text system could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextError {
    /// This OS has no text backend yet.
    Unsupported,
    /// None of the requested families is installed.
    NoFont,
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("text rendering is not supported on this OS yet"),
            Self::NoFont => f.write_str("none of the requested fonts is installed"),
        }
    }
}

impl std::error::Error for TextError {}

/// Shapes and rasterizes text in one primary font, with OS font fallback.
///
/// Used on the UI thread. Shaping a row and rasterizing a glyph are short
/// CPU work; callers cache their results, so neither runs again for text
/// that has not changed.
pub trait TextSystem {
    /// The cell grid of the primary font.
    fn metrics(&self) -> CellMetrics;

    /// Shapes one row of text into `out`, replacing its contents. Characters
    /// the primary font lacks come from fallback fonts. Line breaks and tabs
    /// are not handled: the caller passes one row of plain text.
    fn shape(&mut self, text: &str, out: &mut Vec<ShapedGlyph>);

    /// Rasterizes a glyph from [`Self::shape`]. `None` for glyphs that draw
    /// nothing, such as a space, and for keys this system did not return.
    fn rasterize(&mut self, key: GlyphKey) -> Option<GlyphBitmap>;
}

/// Loads the first installed family of `request` at `scale` physical pixels
/// per point.
///
/// This reads font files, so it is done once at startup and when the scale
/// factor changes, not per frame.
pub fn load(request: &FontRequest<'_>, scale: f64) -> Result<Box<dyn TextSystem>, TextError> {
    #[cfg(target_os = "macos")]
    {
        coretext::CoreTextSystem::load(request, scale)
            .map(|system| Box::new(system) as Box<dyn TextSystem>)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (request, scale);
        Err(TextError::Unsupported)
    }
}

/// A text system that shapes nothing: for systems without a backend, or
/// when no font could be loaded. The grid still has cells, so a cursor can
/// be drawn.
#[derive(Debug, Clone, Copy)]
pub struct NullTextSystem {
    metrics: CellMetrics,
}

impl NullTextSystem {
    /// A grid with cells estimated from the font size.
    pub fn new(size_pt: f32, scale: f64) -> Self {
        Self {
            metrics: CellMetrics::estimate(size_pt, scale),
        }
    }
}

impl TextSystem for NullTextSystem {
    fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    fn shape(&mut self, _text: &str, out: &mut Vec<ShapedGlyph>) {
        out.clear();
    }

    fn rasterize(&mut self, _key: GlyphKey) -> Option<GlyphBitmap> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimated_cells_are_whole_pixels() {
        for scale in [1.0, 1.5, 2.0] {
            let m = CellMetrics::estimate(13.0, scale);
            assert_eq!(m.width, m.width.round());
            assert_eq!(m.height, m.height.round());
            assert_eq!(m.ascent + m.descent, m.height);
            assert!(m.width >= 1.0 && m.ascent <= m.height);
        }
    }

    #[test]
    fn estimated_cells_scale_with_the_display() {
        let one = CellMetrics::estimate(13.0, 1.0);
        let two = CellMetrics::estimate(13.0, 2.0);
        assert!(two.width > one.width && two.height > one.height);
    }

    #[test]
    fn null_system_shapes_nothing() {
        let mut system = NullTextSystem::new(13.0, 2.0);
        let mut out = vec![ShapedGlyph {
            key: GlyphKey {
                font: FontId(0),
                glyph: GlyphId(1),
            },
            column: 0,
            cells: 1,
            cluster: 0,
            offset: [0.0, 0.0],
        }];
        system.shape("abc", &mut out);
        assert!(out.is_empty());
        assert_eq!(
            system.rasterize(GlyphKey {
                font: FontId(0),
                glyph: GlyphId(1)
            }),
            None
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn other_systems_are_unsupported() {
        let request = FontRequest {
            families: &["Menlo"],
            size_pt: 13.0,
        };
        assert_eq!(load(&request, 1.0).err(), Some(TextError::Unsupported));
    }
}
