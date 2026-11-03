//! The CoreText text system (macOS, ADR 0036).
//!
//! All `unsafe` code of the text system is in this file: thin calls into
//! CoreText and CoreGraphics through the objc2 bindings. The bindings mark
//! even plain getters `unsafe`, so most blocks only need a valid object.
//!
//! Font fallback is CoreText's own: a `CTLine` splits text into runs, and a
//! run whose characters the primary font lacks uses a font from the
//! system's cascade list for the primary font (CJK, emoji and so on).

use std::ptr::{self, NonNull};

use objc2_core_foundation::{
    CFArray, CFAttributedString, CFDictionary, CFIndex, CFRange, CFRetained, CFString, CFType,
    CGFloat, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGGlyph, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_core_text::{
    CTFont, CTFontOrientation, CTFontSymbolicTraits, CTLine, CTRun, kCTFontAttributeName,
};

use super::grid::{self, RawGlyph};
use super::{
    CellMetrics, FontId, FontRequest, GlyphBitmap, GlyphFormat, GlyphId, GlyphKey, ShapedGlyph,
    TextError, TextSystem,
};

/// Empty pixels around a glyph's bounds, so antialiased edges are not cut.
const PADDING: i32 = 1;

/// The largest glyph bitmap side, in pixels. Larger glyphs (from a broken or
/// hostile font) are not drawn.
const MAX_GLYPH_SIDE: u32 = 1024;

/// A font the system has handed out a [`FontId`] for.
struct LoadedFont {
    font: CFRetained<CTFont>,
    /// Whether its glyphs are color bitmaps (emoji).
    color: bool,
}

impl LoadedFont {
    fn new(font: CFRetained<CTFont>) -> Self {
        // SAFETY: `font` is a valid font.
        let traits = unsafe { font.symbolic_traits() };
        Self {
            color: traits.contains(CTFontSymbolicTraits::TraitColorGlyphs),
            font,
        }
    }
}

/// Shapes and rasterizes text with CoreText.
pub(super) struct CoreTextSystem {
    /// Index is the [`FontId`]. 0 is the primary font, the rest are fallback
    /// fonts in the order shaping first used them.
    fonts: Vec<LoadedFont>,
    metrics: CellMetrics,
    /// Scratch buffers reused across calls to `shape`.
    raw: Vec<RawGlyph>,
    glyphs: Vec<CGGlyph>,
    positions: Vec<CGPoint>,
    advances: Vec<CGSize>,
    indices: Vec<CFIndex>,
}

impl CoreTextSystem {
    pub(super) fn load(request: &FontRequest<'_>, scale: f64) -> Result<Self, TextError> {
        let size = CGFloat::from(request.size_pt) * scale;
        let font = request
            .families
            .iter()
            .find_map(|family| font_named(family, size))
            .ok_or(TextError::NoFont)?;
        let metrics = cell_metrics(&font, size);
        Ok(Self {
            fonts: vec![LoadedFont::new(font)],
            metrics,
            raw: Vec::new(),
            glyphs: Vec::new(),
            positions: Vec::new(),
            advances: Vec::new(),
            indices: Vec::new(),
        })
    }

    /// The id of the font a run was shaped with, registering a fallback
    /// font the first time it is seen.
    fn run_font(&mut self, run: &CTRun) -> FontId {
        // SAFETY: `run` is a valid run.
        let attributes = unsafe { run.attributes() };
        // SAFETY: the keys of a run's attribute dictionary are attribute
        // names, which are strings, and every value is a CF object.
        let attributes: CFRetained<CFDictionary<CFString, CFType>> =
            unsafe { CFRetained::cast_unchecked(attributes) };
        let Some(font) = attributes
            .get(font_attribute())
            .and_then(|value| value.downcast::<CTFont>().ok())
        else {
            return FontId(0);
        };
        if let Some(i) = self.fonts.iter().position(|loaded| loaded.font == font) {
            return FontId(i as u16);
        }
        let Ok(id) = u16::try_from(self.fonts.len()) else {
            return FontId(0);
        };
        self.fonts.push(LoadedFont::new(font));
        FontId(id)
    }

    /// Appends the glyphs of one run to `self.raw`.
    fn collect_run(&mut self, run: &CTRun) {
        // SAFETY: `run` is a valid run.
        let count = unsafe { run.glyph_count() };
        let Ok(n) = usize::try_from(count) else {
            return;
        };
        if n == 0 {
            return;
        }
        let font = self.run_font(run);
        self.glyphs.resize(n, 0);
        self.positions.resize(n, CGPoint::ZERO);
        self.advances.resize(n, CGSize::ZERO);
        self.indices.resize(n, 0);
        let all = CFRange {
            location: 0,
            length: count,
        };
        // SAFETY: each buffer holds exactly `count` elements, the run's glyph
        // count, and `all` asks for exactly that many.
        unsafe {
            run.glyphs(all, buffer(&mut self.glyphs));
            run.positions(all, buffer(&mut self.positions));
            run.advances(all, buffer(&mut self.advances));
            run.string_indices(all, buffer(&mut self.indices));
        }
        for i in 0..n {
            let Ok(utf16) = usize::try_from(self.indices[i]) else {
                continue;
            };
            self.raw.push(RawGlyph {
                key: GlyphKey {
                    font,
                    glyph: GlyphId(self.glyphs[i]),
                },
                utf16,
                x: self.positions[i].x as f32,
                y: self.positions[i].y as f32,
                advance: self.advances[i].width as f32,
            });
        }
    }
}

impl TextSystem for CoreTextSystem {
    fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    fn shape(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
        out.clear();
        self.raw.clear();
        if text.is_empty() {
            return;
        }
        let string = CFString::from_str(text);
        let primary: &CFType = &self.fonts[0].font;
        let attributes =
            CFDictionary::<CFString, CFType>::from_slices(&[font_attribute()], &[primary]);
        // SAFETY: the default allocator, a valid string, and attributes that
        // map the font attribute name to a font, as CoreText expects.
        let attributed =
            unsafe { CFAttributedString::new(None, Some(&string), Some(attributes.as_ref())) };
        let Some(attributed) = attributed else {
            return;
        };
        // SAFETY: `attributed` is a valid attributed string.
        let line = unsafe { CTLine::with_attributed_string(&attributed) };
        // SAFETY: `line` is a valid line.
        let runs = unsafe { line.glyph_runs() };
        // SAFETY: a CFArray holds CF objects; each is checked to be a run
        // before use.
        let runs: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(runs) };
        for run in runs.iter() {
            if let Ok(run) = run.downcast::<CTRun>() {
                self.collect_run(&run);
            }
        }
        grid::place(text, &self.raw, self.metrics.width, out);
    }

    fn rasterize(&mut self, key: GlyphKey) -> Option<GlyphBitmap> {
        let loaded = self.fonts.get(usize::from(key.font.0))?;
        let glyph: CGGlyph = key.glyph.0;
        let mut bounds = CGRect::ZERO;
        // SAFETY: one glyph and room for one rect, matching the count of 1.
        unsafe {
            loaded.font.bounding_rects_for_glyphs(
                CTFontOrientation::Default,
                NonNull::from(&glyph),
                &mut bounds,
                1,
            );
        }
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return None;
        }
        let left = bounds.origin.x.floor() as i32 - PADDING;
        let bottom = bounds.origin.y.floor() as i32 - PADDING;
        let right = (bounds.origin.x + bounds.size.width).ceil() as i32 + PADDING;
        let top = (bounds.origin.y + bounds.size.height).ceil() as i32 + PADDING;
        let width = u32::try_from(right - left).ok()?;
        let height = u32::try_from(top - bottom).ok()?;
        if width > MAX_GLYPH_SIDE || height > MAX_GLYPH_SIDE {
            return None;
        }

        let format = if loaded.color {
            GlyphFormat::Color
        } else {
            GlyphFormat::Coverage
        };
        let stride = width as usize * format.bytes_per_pixel();
        let mut data = vec![0u8; stride * height as usize];
        let (space, info) = match format {
            // Alpha only: one byte of coverage per pixel, no color space.
            GlyphFormat::Coverage => (None, CGImageAlphaInfo::Only.0),
            // RGBA in memory order, premultiplied.
            GlyphFormat::Color => (
                // SAFETY: an immutable constant CoreGraphics exports.
                CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })),
                CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
            ),
        };
        {
            // SAFETY: `data` holds `stride * height` bytes, which is what a
            // `width` x `height` context with 8-bit components and `stride`
            // bytes per row writes. `data` outlives the context, which is
            // dropped at the end of this block, before `data` is read.
            let context = unsafe {
                CGBitmapContextCreate(
                    data.as_mut_ptr().cast(),
                    width as usize,
                    height as usize,
                    8,
                    stride,
                    space.as_deref(),
                    info,
                )
            }?;
            CGContext::set_allows_font_smoothing(Some(&context), false);
            CGContext::set_should_antialias(Some(&context), true);
            CGContext::set_gray_fill_color(Some(&context), 1.0, 1.0);
            // The bitmap's bottom-left corner is the origin, and the pen
            // sits `left` and `bottom` pixels inside it.
            let position = CGPoint::new(-CGFloat::from(left), -CGFloat::from(bottom));
            // SAFETY: one glyph and one position, matching the count of 1.
            unsafe {
                loaded.font.draw_glyphs(
                    NonNull::from(&glyph),
                    NonNull::from(&position),
                    1,
                    &context,
                );
            }
        }
        Some(GlyphBitmap {
            width,
            height,
            left,
            top,
            format,
            data,
        })
    }
}

/// `kCTFontAttributeName`: the attribute whose value is a run's font.
fn font_attribute() -> &'static CFString {
    // SAFETY: an immutable constant CoreText exports.
    unsafe { kCTFontAttributeName }
}

/// A pointer for CoreText to fill `buffer` through.
fn buffer<T>(buffer: &mut [T]) -> NonNull<T> {
    NonNull::from(buffer).cast()
}

/// The font of `family` at `size` pixels, or `None` if it is not installed.
fn font_named(family: &str, size: CGFloat) -> Option<CFRetained<CTFont>> {
    let name = CFString::from_str(family);
    // SAFETY: a null matrix is documented to mean the identity.
    let font = unsafe { CTFont::with_name(&name, size, ptr::null()) };
    // CoreText substitutes another font for a missing one; reject that.
    // SAFETY: `font` is a valid font.
    let got = unsafe { font.family_name() }.to_string();
    got.eq_ignore_ascii_case(family).then_some(font)
}

/// The cell grid of a monospace font at `size` pixels, in whole pixels.
fn cell_metrics(font: &CTFont, size: CGFloat) -> CellMetrics {
    // SAFETY: `font` is a valid font.
    let (ascent, descent, leading) = unsafe { (font.ascent(), font.descent(), font.leading()) };
    let advance = advance_of(font, 'M').unwrap_or(size * 0.6);
    let width = advance.round().max(1.0);
    let height = (ascent + descent + leading).ceil().max(1.0);
    let ascent = (ascent + leading / 2.0).round().clamp(0.0, height);
    CellMetrics {
        width: width as f32,
        height: height as f32,
        ascent: ascent as f32,
        descent: (height - ascent) as f32,
    }
}

/// The advance of `c` in `font`, if the font has a glyph for it.
fn advance_of(font: &CTFont, c: char) -> Option<CGFloat> {
    let mut units = [0u16; 2];
    let units = c.encode_utf16(&mut units);
    let mut glyphs: [CGGlyph; 2] = [0; 2];
    // SAFETY: `units` and `glyphs` both hold `units.len()` elements.
    let found = unsafe {
        font.glyphs_for_characters(buffer(units), buffer(&mut glyphs), units.len() as CFIndex)
    };
    if !found {
        return None;
    }
    // SAFETY: one glyph; a null advances buffer is allowed, and only the
    // total is returned.
    let advance = unsafe {
        font.advances_for_glyphs(
            CTFontOrientation::Horizontal,
            buffer(&mut glyphs),
            ptr::null_mut(),
            1,
        )
    };
    Some(advance)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: FontRequest<'static> = FontRequest {
        families: &["Menlo"],
        size_pt: 13.0,
    };

    fn menlo(scale: f64) -> CoreTextSystem {
        CoreTextSystem::load(&REQUEST, scale).expect("Menlo ships with macOS")
    }

    #[test]
    fn missing_family_falls_back_to_the_next() {
        let request = FontRequest {
            families: &["No Such Font oso", "Menlo"],
            size_pt: 13.0,
        };
        let system = CoreTextSystem::load(&request, 1.0).unwrap();
        assert_eq!(system.metrics(), menlo(1.0).metrics());
    }

    #[test]
    fn no_installed_family_is_an_error() {
        let request = FontRequest {
            families: &["No Such Font oso"],
            size_pt: 13.0,
        };
        assert_eq!(
            CoreTextSystem::load(&request, 1.0).err(),
            Some(TextError::NoFont)
        );
    }

    #[test]
    fn cells_are_whole_pixels_and_scale() {
        let one = menlo(1.0).metrics();
        let two = menlo(2.0).metrics();
        for m in [one, two] {
            assert_eq!(m.width, m.width.round());
            assert_eq!(m.height, m.height.round());
            assert_eq!(m.ascent + m.descent, m.height);
        }
        assert!(two.width >= 2.0 * one.width - 1.0);
        assert!(two.height >= 2.0 * one.height - 1.0);
    }

    #[test]
    fn full_width_and_emoji_take_two_cells() {
        let mut system = menlo(2.0);
        let mut out = Vec::new();
        system.shape("a日🍣b", &mut out);
        let cells: Vec<_> = out.iter().map(|g| (g.column, g.cells)).collect();
        assert_eq!(cells, [(0, 1), (1, 2), (3, 2), (5, 1)]);
        // 日 and 🍣 come from fallback fonts.
        assert_eq!(out[0].key.font, FontId(0));
        assert_ne!(out[1].key.font, FontId(0));
        assert_ne!(out[2].key.font, FontId(0));
        assert_eq!(out[3].key.font, FontId(0));
    }

    #[test]
    fn fallback_fonts_are_reused() {
        let mut system = menlo(1.0);
        let mut out = Vec::new();
        system.shape("日", &mut out);
        let first = out[0].key.font;
        system.shape("本", &mut out);
        assert_eq!(out[0].key.font, first);
    }

    #[test]
    fn text_is_coverage_and_emoji_is_color() {
        let mut system = menlo(2.0);
        let mut out = Vec::new();
        system.shape("M🍣", &mut out);
        let text = system.rasterize(out[0].key).unwrap();
        let emoji = system.rasterize(out[1].key).unwrap();
        assert_eq!(text.format, GlyphFormat::Coverage);
        assert_eq!(emoji.format, GlyphFormat::Color);
        for bitmap in [&text, &emoji] {
            let len = bitmap.width * bitmap.height * bitmap.format.bytes_per_pixel() as u32;
            assert_eq!(bitmap.data.len(), len as usize);
            assert!(bitmap.data.iter().any(|&b| b != 0), "nothing drawn");
        }
        // Emoji pixels have color: some red, green and blue differ.
        assert!(
            emoji
                .data
                .chunks_exact(4)
                .any(|p| p[0] != p[1] || p[1] != p[2])
        );
    }

    #[test]
    fn rows_run_top_to_bottom() {
        // T has its bar at the top and its stem below.
        let mut system = menlo(2.0);
        let mut out = Vec::new();
        system.shape("T", &mut out);
        let t = system.rasterize(out[0].key).unwrap();
        let row_ink = |y: u32| -> u32 {
            let start = (y * t.width) as usize;
            t.data[start..start + t.width as usize]
                .iter()
                .map(|&b| u32::from(b))
                .sum()
        };
        let ink: Vec<u32> = (0..t.height).map(row_ink).collect();
        let densest = ink.iter().enumerate().max_by_key(|(_, v)| **v).unwrap().0;
        assert!(densest < t.height as usize / 2, "{ink:?}");
        assert!(t.top > 0, "T sits above the baseline");
    }

    #[test]
    fn space_draws_nothing() {
        let mut system = menlo(1.0);
        let mut out = Vec::new();
        system.shape(" ", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(system.rasterize(out[0].key), None);
    }

    #[test]
    fn unknown_font_draws_nothing() {
        let mut system = menlo(1.0);
        let key = GlyphKey {
            font: FontId(999),
            glyph: GlyphId(1),
        };
        assert_eq!(system.rasterize(key), None);
    }
}
