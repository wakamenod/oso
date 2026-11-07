//! The CoreText text system (macOS, ADR 0036).
//!
//! Must not depend on: `control`, `oso-core` or the GPU. CoreText types do not
//! leave this file.
//!
//! All `unsafe` code of the text system is in this file: thin calls into
//! CoreText and CoreGraphics through the objc2 bindings. The bindings mark
//! even plain getters `unsafe`, so most blocks only need a valid object.
//!
//! Font fallback is CoreText's own: a `CTLine` splits text into runs, and a
//! run whose characters the primary font lacks uses a font from the
//! system's cascade list for the primary font (CJK, emoji and so on).
//!
//! Lines come from a `CTTypesetter` with a forced embedding level, so no
//! character is shown as its mirrored form (ADR 0066). A row is split by the
//! direction of its scripts. Right-to-left scripts are shaped at level 1, since
//! their marks and joining need it, and no character there has a mirrored form.
//! Everything else is shaped at level 0.

use std::ptr::{self, NonNull};

use objc2_core_foundation::{
    CFArray, CFAttributedString, CFBoolean, CFDictionary, CFIndex, CFNumber, CFRange, CFRetained,
    CFString, CFType, CGFloat, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGGlyph, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_core_text::{
    CTFont, CTFontOrientation, CTFontSymbolicTraits, CTRun, CTTypesetter, kCTFontAttributeName,
    kCTTypesetterOptionAllowUnboundedLayout, kCTTypesetterOptionForcedEmbeddingLevel,
};

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
    /// What text is shaped with, kept since `shape` runs per piece: the
    /// primary font, and the options of embedding level 0 and 1.
    attributes: CFRetained<CFDictionary<CFString, CFType>>,
    level_0: CFRetained<CFDictionary<CFString, CFType>>,
    level_1: CFRetained<CFDictionary<CFString, CFType>>,
    /// Scratch buffers reused across calls to `shape`. `bytes` maps UTF-16
    /// offsets, which CoreText uses, to byte offsets.
    bytes: Vec<u32>,
    glyphs: Vec<CGGlyph>,
    positions: Vec<CGPoint>,
    advances: Vec<CGSize>,
    indices: Vec<CFIndex>,
}

impl CoreTextSystem {
    pub(super) fn load(request: &FontRequest<'_>, scale: f64) -> Result<Self, TextError> {
        let size = CGFloat::from(super::font_size(request.size_pt)) * super::display_scale(scale);
        let font = request
            .families
            .iter()
            .find_map(|family| font_named(family, size))
            .ok_or(TextError::NoFont)?;
        let metrics = cell_metrics(&font, size);
        let primary: &CFType = &font;
        let attributes = CFDictionary::from_slices(&[font_attribute()], &[primary]);
        Ok(Self {
            attributes,
            fonts: vec![LoadedFont::new(font)],
            metrics,
            level_0: typesetter_options(0),
            level_1: typesetter_options(1),
            bytes: Vec::new(),
            glyphs: Vec::new(),
            positions: Vec::new(),
            advances: Vec::new(),
            indices: Vec::new(),
        })
    }

    /// The id of the font a run was shaped with, registering a fallback
    /// font the first time it is seen. `None` if the run names no font, or
    /// there are too many fonts: its glyph ids would mean nothing.
    fn run_font(&mut self, run: &CTRun) -> Option<FontId> {
        // SAFETY: `run` is a valid run.
        let attributes = unsafe { run.attributes() };
        // SAFETY: the keys of a run's attribute dictionary are attribute
        // names, which are strings, and every value is a CF object. Values
        // are only used after a checked downcast.
        let attributes: CFRetained<CFDictionary<CFString, CFType>> =
            unsafe { CFRetained::cast_unchecked(attributes) };
        let font = attributes
            .get(font_attribute())
            .and_then(|value| value.downcast::<CTFont>().ok())?;
        if let Some(i) = self.fonts.iter().position(|loaded| loaded.font == font) {
            return u16::try_from(i).ok().map(FontId);
        }
        let id = u16::try_from(self.fonts.len()).ok()?;
        self.fonts.push(LoadedFont::new(font));
        Some(FontId(id))
    }

    /// Appends the glyphs of one run to `out`.
    fn collect_run(&mut self, run: &CTRun, out: &mut Vec<ShapedGlyph>) {
        // SAFETY: `run` is a valid run.
        let count = unsafe { run.glyph_count() };
        let Ok(n) = usize::try_from(count) else {
            return;
        };
        if n == 0 {
            return;
        }
        let Some(font) = self.run_font(run) else {
            return;
        };
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
            let byte = usize::try_from(self.indices[i])
                .ok()
                .and_then(|utf16| self.bytes.get(utf16));
            let Some(&cluster) = byte else {
                continue;
            };
            out.push(ShapedGlyph {
                key: GlyphKey {
                    font,
                    glyph: GlyphId(self.glyphs[i]),
                },
                cluster,
                x: self.positions[i].x as f32,
                y: self.positions[i].y as f32,
                advance: self.advances[i].width as f32,
            });
        }
    }

    /// Appends the glyphs of `text`, of one direction, which starts `start`
    /// bytes into the row and `pen` pixels along it, and returns their total
    /// advance.
    fn shape_direction(
        &mut self,
        text: &str,
        start: usize,
        right_to_left: bool,
        pen: f32,
        out: &mut Vec<ShapedGlyph>,
    ) -> f32 {
        self.bytes.clear();
        for (byte, c) in text.char_indices() {
            let byte = u32::try_from(start + byte).unwrap_or(u32::MAX);
            self.bytes.extend(std::iter::repeat_n(byte, c.len_utf16()));
        }
        let string = CFString::from_str(text);
        // SAFETY: the default allocator, a valid string, and attributes that
        // map the font attribute name to a font, as CoreText expects.
        let attributed =
            unsafe { CFAttributedString::new(None, Some(&string), Some(self.attributes.as_ref())) };
        let Some(attributed) = attributed else {
            return 0.0;
        };
        let options = if right_to_left {
            &self.level_1
        } else {
            &self.level_0
        };
        // SAFETY: `attributed` is a valid attributed string, and `options`
        // maps each option to a value of the type CoreText documents for it.
        let typesetter = unsafe {
            CTTypesetter::with_attributed_string_and_options(&attributed, Some(options.as_ref()))
        };
        let Some(typesetter) = typesetter else {
            return 0.0;
        };
        let all = CFRange {
            location: 0,
            length: 0,
        };
        // SAFETY: `typesetter` is valid, and a length of 0 is documented to mean
        // the whole string.
        let line = unsafe { typesetter.line(all) };
        // SAFETY: `line` is a valid line.
        let runs = unsafe { line.glyph_runs() };
        // SAFETY: a CFArray holds CF objects; each is checked to be a run
        // before use.
        let runs: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(runs) };
        let first = out.len();
        for run in runs.iter() {
            if let Ok(run) = run.downcast::<CTRun>() {
                self.collect_run(&run, out);
            }
        }
        let mut advance = 0.0;
        for glyph in &mut out[first..] {
            glyph.x += pen;
            advance += glyph.advance;
        }
        advance
    }
}

impl TextSystem for CoreTextSystem {
    fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    fn shape(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
        out.clear();
        let mut start = 0;
        let mut pen = 0.0;
        while let Some(first) = text[start..].chars().next() {
            let right_to_left = is_right_to_left(first);
            let len = text[start..]
                .find(|c| !stays_in_run(c) && is_right_to_left(c) != right_to_left)
                .unwrap_or(text.len() - start);
            let end = start + len;
            pen += self.shape_direction(&text[start..end], start, right_to_left, pen, out);
            start = end;
        }
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
        let BitmapBox {
            left,
            bottom,
            width,
            height,
        } = bitmap_box(bounds)?;

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
            top: bottom + height as i32,
            format,
            data,
        })
    }
}

/// The pixels a glyph bitmap covers, around the pen at the origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BitmapBox {
    left: i32,
    bottom: i32,
    width: u32,
    height: u32,
}

/// The whole-pixel box, with padding, around a glyph's `bounds`. `None` for
/// an empty glyph, and for bounds that are not finite or larger than
/// [`MAX_GLYPH_SIDE`] (a broken or hostile font).
fn bitmap_box(bounds: CGRect) -> Option<BitmapBox> {
    let (x, y) = (bounds.origin.x, bounds.origin.y);
    let (w, h) = (bounds.size.width, bounds.size.height);
    if ![x, y, w, h].iter().all(|v| v.is_finite()) || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let padding = CGFloat::from(PADDING);
    let left = x.floor() - padding;
    let bottom = y.floor() - padding;
    let width = (x + w).ceil() + padding - left;
    let height = (y + h).ceil() + padding - bottom;
    let max = CGFloat::from(MAX_GLYPH_SIDE);
    // The pen stays within a few glyph sizes of the bitmap.
    let reach = 4.0 * max;
    if width > max || height > max || left.abs() > reach || bottom.abs() > reach {
        return None;
    }
    Some(BitmapBox {
        left: left as i32,
        bottom: bottom as i32,
        width: width as u32,
        height: height as u32,
    })
}

/// `kCTFontAttributeName`: the attribute whose value is a run's font.
fn font_attribute() -> &'static CFString {
    // SAFETY: an immutable constant CoreText exports.
    unsafe { kCTFontAttributeName }
}

/// Whether `c` is in a block of a right-to-left script. Its letters need
/// embedding level 1 to shape as they should. None of these characters has a
/// mirrored form (`Bidi_Mirrored` in Unicode 17), so that level cannot show one.
fn is_right_to_left(c: char) -> bool {
    matches!(
        u32::from(c),
        0x590..=0x8FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF
    )
}

/// Whether `c` stays in the run before it. A mark or a joiner is shaped with its
/// letter, or the font cannot place it there, and a space saves a run per word.
/// None of these has a mirrored form either.
fn stays_in_run(c: char) -> bool {
    matches!(
        u32::from(c),
        0x20 | 0x200C..=0x200D
            | 0x300..=0x36F
            | 0x591..=0x5C7
            | 0x610..=0x61A
            | 0x64B..=0x65F
            | 0x670
            | 0x6D6..=0x6ED
            | 0x711
            | 0x730..=0x74A
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE00..=0xFE0F
            | 0xFE20..=0xFE2F
    )
}

/// A forced embedding `level`, so no bidirectional algorithm runs, and no limit
/// on a line's length from CoreText: `oso-render` bounds what it shapes per row.
fn typesetter_options(level: i32) -> CFRetained<CFDictionary<CFString, CFType>> {
    let level = CFNumber::new_i32(level);
    let level: &CFType = &level;
    let unbounded: &CFType = CFBoolean::new(true);
    // SAFETY: immutable constants CoreText exports.
    let keys = unsafe {
        [
            kCTTypesetterOptionForcedEmbeddingLevel,
            kCTTypesetterOptionAllowUnboundedLayout,
        ]
    };
    CFDictionary::from_slices(&keys, &[level, unbounded])
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
    fn fallback_fonts_draw_what_menlo_lacks() {
        let mut system = menlo(2.0);
        let mut out = Vec::new();
        system.shape("a日🍣b", &mut out);
        let clusters: Vec<_> = out.iter().map(|g| g.cluster).collect();
        assert_eq!(clusters, [0, 1, 4, 8], "byte offsets, not UTF-16");
        // 日 and 🍣 come from fallback fonts.
        assert_eq!(out[0].key.font, FontId(0));
        assert_ne!(out[1].key.font, FontId(0));
        assert_ne!(out[2].key.font, FontId(0));
        assert_eq!(out[3].key.font, FontId(0));
        // Pen positions advance left to right.
        assert!(out.windows(2).all(|w| w[0].x < w[1].x));
        assert_eq!(out[1].x, out[0].x + out[0].advance);
    }

    /// The glyph `system` gives `c` shaped alone, where nothing can change it.
    fn key_of(system: &mut CoreTextSystem, c: char) -> GlyphKey {
        let mut out = Vec::new();
        system.shape(c.encode_utf8(&mut [0; 4]), &mut out);
        assert_eq!(out.len(), 1, "{c:?}");
        out[0].key
    }

    #[test]
    fn right_to_left_text_shows_no_mirrored_form() {
        let mut system = menlo(1.0);
        let mut out = Vec::new();
        for s in ['\u{5d0}', '\u{5d1}', '\u{633}', '\u{644}'] {
            for neutral in "<>()[]{}«»‹›≤≥".chars() {
                let want = key_of(&mut system, neutral);
                for text in [
                    format!("{s}{neutral}{s} {neutral} {s}"),
                    format!("{s}{s}{neutral}{s}{s}"),
                    format!("{neutral}{s}"),
                    format!("{s}{neutral}"),
                ] {
                    system.shape(&text, &mut out);
                    for (byte, _) in text.match_indices(neutral) {
                        let glyph = out.iter().find(|g| g.cluster == byte as u32);
                        let glyph = glyph.unwrap_or_else(|| panic!("{text:?} {byte}"));
                        assert_eq!(glyph.key, want, "{text:?}");
                    }
                }
            }
        }
    }

    /// The pen positions of the glyphs of the characters `at` of `text`.
    fn pens_at(system: &mut CoreTextSystem, text: &str, at: &[usize]) -> Vec<f32> {
        let mut out = Vec::new();
        system.shape(text, &mut out);
        at.iter()
            .map(|&byte| {
                let glyph = out.iter().find(|g| g.cluster == byte as u32);
                glyph
                    .unwrap_or_else(|| panic!("{text:?} {byte}: {out:?}"))
                    .x
            })
            .collect()
    }

    #[test]
    fn pens_follow_the_order_of_the_text_across_directions() {
        let mut system = menlo(1.0);
        // a, the Hebrew letter, <, the next Hebrew letter and b.
        let pens = pens_at(&mut system, "a \u{5d0} <\u{5d1}b ", &[0, 2, 5, 6, 8]);
        assert!(pens.is_sorted_by(|a, b| a < b), "{pens:?}");
    }

    #[test]
    fn every_letter_with_marks_has_a_glyph_of_its_own() {
        let mut system = menlo(1.0);
        let mut out = Vec::new();
        let marks = [
            "\u{5b0}", "\u{5bc}", "\u{5c1}", "\u{596}", "\u{64e}", "\u{651}", "\u{301}",
        ];
        for letter in [
            "\u{5d1}", "\u{5e8}", "\u{5e9}", "\u{633}", "\u{71d}", "a", "\u{3b1}",
        ] {
            for next in ["\u{5e8}", "\u{644}", "a"] {
                for first in marks {
                    for second in marks {
                        let text = format!("{letter}{first}{second}{next}");
                        let next = text.len() - next.len();
                        system.shape(&text, &mut out);
                        for byte in [0, next] {
                            assert!(
                                out.iter().any(|g| g.cluster == byte as u32),
                                "{text:?} {byte}: {out:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_mark_or_a_joiner_stays_on_its_letter() {
        let mut system = menlo(1.0);
        let cell = system.metrics().width;
        let mut out = Vec::new();
        for (letter, after) in [
            ("\u{5d0}", "\u{301}"),
            ("\u{5d0}", "\u{308}"),
            ("\u{710}", "\u{308}"),
            ("\u{6cc}", "\u{200c}"),
            ("\u{644}", "\u{200d}"),
            ("\u{5d0}", "\u{fe00}"),
        ] {
            let text = format!("{letter}{after}");
            system.shape(&text, &mut out);
            let total: f32 = out.iter().map(|g| g.advance).sum();
            assert!(total < 1.5 * cell, "{text:?}: {out:?}");
            for glyph in &out {
                assert!(glyph.x < cell, "{text:?}: {out:?}");
            }
        }
    }

    #[test]
    fn only_characters_with_no_mirrored_form_can_share_a_right_to_left_run() {
        // The closest characters with a mirrored form to the ranges, and ones
        // that look as if they had one.
        for c in [
            '(',
            ')',
            '<',
            '>',
            '[',
            ']',
            '{',
            '}',
            '\u{ab}',
            '\u{bb}',
            '\u{2039}',
            '\u{2264}',
            '\u{fe59}',
            '\u{fe5e}',
            '\u{fe64}',
            '\u{fe65}',
            '\u{ff08}',
            '\u{ff09}',
            '\u{1d6db}',
            '\u{1d7c3}',
        ] {
            assert!(!is_right_to_left(c), "{c:?}");
            assert!(!stays_in_run(c), "{c:?}");
        }
        assert!(is_right_to_left('\u{fd3e}') && is_right_to_left('\u{fd3f}'));
        // The edges of the ranges.
        for c in [
            '\u{58f}',
            '\u{900}',
            '\u{fb1c}',
            '\u{fe6f}',
            '\u{ff00}',
            '\u{1e7ff}',
            '\u{1f000}',
        ] {
            assert!(!is_right_to_left(c), "{c:?}");
        }
        for c in [
            '\u{590}',
            '\u{8ff}',
            '\u{fb1d}',
            '\u{fe70}',
            '\u{10800}',
            '\u{1efff}',
        ] {
            assert!(is_right_to_left(c), "{c:?}");
        }
    }

    #[test]
    fn digits_stay_where_the_text_has_them_after_right_to_left_text() {
        let mut system = menlo(1.0);
        // The digits and the < of the text, in its order.
        let pens = pens_at(&mut system, "\u{5d0} 12 < 34", &[3, 4, 6, 8, 9]);
        assert!(pens.is_sorted_by(|a, b| a < b), "{pens:?}");
    }

    #[test]
    fn a_very_long_row_is_shaped_whole() {
        // What bounds a row is the caller's budget, not this system.
        let mut system = menlo(1.0);
        let mut out = Vec::new();
        for unit in ["a", "\u{5d0}"] {
            system.shape(&unit.repeat(11_000), &mut out);
            assert_eq!(out.len(), 11_000, "{unit:?}");
        }
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

    fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
    }

    #[test]
    fn bitmap_box_pads_whole_pixels() {
        let b = bitmap_box(rect(0.5, -2.25, 7.0, 10.0)).unwrap();
        assert_eq!(
            b,
            BitmapBox {
                left: -1,
                bottom: -4,
                width: 10,
                height: 13,
            }
        );
    }

    #[test]
    fn bitmap_box_refuses_bad_bounds() {
        for bounds in [
            rect(0.0, 0.0, 0.0, 10.0),
            rect(0.0, 0.0, 10.0, -1.0),
            rect(f64::NAN, 0.0, 10.0, 10.0),
            rect(0.0, f64::INFINITY, 10.0, 10.0),
            rect(0.0, 0.0, f64::INFINITY, 10.0),
            rect(-1e300, 0.0, 10.0, 10.0),
            rect(0.0, 1e300, 10.0, 10.0),
            rect(0.0, 0.0, 1e300, 10.0),
            rect(0.0, 0.0, f64::from(MAX_GLYPH_SIDE), 10.0),
        ] {
            assert_eq!(bitmap_box(bounds), None, "{bounds:?}");
        }
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
