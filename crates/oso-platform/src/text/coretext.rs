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
//! character is shown as its mirrored form (ADR 0066). A row of both directions
//! is split into runs by `direction`, and is at most two lines.

use std::ptr::{self, NonNull};

#[cfg(test)]
use unicode_segmentation::UnicodeSegmentation;

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

mod direction;

use direction::{DirectionRun, SEPARATOR, is_right_to_left, push_bytes};

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
    /// offsets, which CoreText uses, to byte offsets, or to [`SEPARATOR`].
    bytes: Vec<u32>,
    /// The text of one line, and the runs of one direction of a row: where
    /// each starts, whether it is right to left, and its glyphs' leftmost pen
    /// and total advance.
    line: String,
    runs: Vec<DirectionRun>,
    /// Whether a separator drew in the lines of a row, and scratch space for
    /// the clusters drawn, to tell whether a joined row came out whole.
    separator_drew: bool,
    drawn: Vec<u32>,
    /// Lines made so far, for tests.
    #[cfg(test)]
    lines: usize,
    /// For tests: a separator counts as drawing, as in a font that credits it a
    /// glyph.
    #[cfg(test)]
    separator_draws: bool,
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
            line: String::new(),
            runs: Vec::new(),
            separator_drew: false,
            drawn: Vec::new(),
            #[cfg(test)]
            lines: 0,
            #[cfg(test)]
            separator_draws: false,
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
            // A separator between runs draws nothing. If its glyph has an
            // advance, the font made something of it, and the row is shaped
            // again with its runs apart.
            if byte == Some(&SEPARATOR) {
                #[cfg(test)]
                if self.separator_draws {
                    self.separator_drew = true;
                }
                if self.advances[i].width != 0.0 {
                    self.separator_drew = true;
                }
                continue;
            }
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

    /// Appends the glyphs of `text`, a row of one direction.
    fn shape_direction(&mut self, text: &str, right_to_left: bool, out: &mut Vec<ShapedGlyph>) {
        self.bytes.clear();
        push_bytes(&mut self.bytes, text, 0);
        let line = CFString::from_str(text);
        self.shape_line(&line, right_to_left, out);
    }

    /// Appends the glyphs of the runs of `text` in one direction as one line.
    fn shape_runs(&mut self, text: &str, right_to_left: bool, out: &mut Vec<ShapedGlyph>) {
        direction::join(
            text,
            &self.runs,
            right_to_left,
            &mut self.line,
            &mut self.bytes,
        );
        if !self.line.is_empty() {
            let line = CFString::from_str(&self.line);
            self.shape_line(&line, right_to_left, out);
        }
    }

    /// Appends the glyphs of each run of `text` as a line of its own: what a row
    /// whose joined lines did not come out whole is shaped as.
    fn shape_apart(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
        for index in 0..self.runs.len() {
            let DirectionRun {
                start,
                right_to_left,
                ..
            } = self.runs[index];
            let end = self.runs.get(index + 1).map_or(text.len(), |run| run.start);
            self.bytes.clear();
            push_bytes(&mut self.bytes, &text[start..end], start);
            let line = CFString::from_str(&text[start..end]);
            self.shape_line(&line, right_to_left, out);
        }
    }

    /// Appends the glyphs of `text` as one line at the embedding level of its
    /// direction. `bytes` maps it to the row.
    fn shape_line(&mut self, text: &CFString, right_to_left: bool, out: &mut Vec<ShapedGlyph>) {
        // SAFETY: the default allocator, a valid string, and attributes that
        // map the font attribute name to a font, as CoreText expects.
        let attributed =
            unsafe { CFAttributedString::new(None, Some(text), Some(self.attributes.as_ref())) };
        let Some(attributed) = attributed else {
            return;
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
            return;
        };
        #[cfg(test)]
        {
            self.lines += 1;
        }
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
        for run in runs.iter() {
            if let Ok(run) = run.downcast::<CTRun>() {
                self.collect_run(&run, out);
            }
        }
    }
}

impl TextSystem for CoreTextSystem {
    fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    fn shape(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
        out.clear();
        if text.is_empty() {
            return;
        }
        if text.is_ascii() || !text.chars().any(is_right_to_left) {
            self.shape_direction(text, false, out);
            return;
        }
        direction::split(text, &mut self.runs);
        if let [run] = self.runs[..] {
            self.shape_direction(text, run.right_to_left, out);
            return;
        }
        self.separator_drew = false;
        self.shape_runs(text, false, out);
        self.shape_runs(text, true, out);
        if self.separator_drew
            || !direction::every_run_drawn(text, &self.runs, out, &mut self.drawn)
        {
            out.clear();
            self.shape_apart(text, out);
        }
        direction::line_up(&mut self.runs, out);
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
    use super::direction::grapheme_direction;
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

    /// Every character that `prefix` then the character makes one grapheme of,
    /// from the segmentation tables and no list here.
    fn joining_after(prefix: char) -> impl Iterator<Item = char> {
        ('\u{80}'..=char::MAX).filter(move |c| {
            let text = format!("{prefix}{c}");
            text.graphemes(true).nth(1).is_none()
        })
    }

    /// Every character that makes one grapheme with the letter after it.
    fn prepended() -> impl Iterator<Item = char> {
        ('\u{80}'..=char::MAX).filter(|c| {
            let text = format!("{c}a");
            text.graphemes(true).nth(1).is_none()
        })
    }

    #[test]
    fn a_grapheme_is_never_split_by_direction() {
        let mut system = menlo(1.0);
        let (mut whole, mut split) = (Vec::new(), Vec::new());
        let marks: Vec<char> = joining_after('a').collect();
        assert!(marks.len() > 2000, "{}", marks.len());
        for letter in ['\u{5d0}', '\u{633}', 'a', '\u{3b1}'] {
            let right_to_left = is_right_to_left(letter);
            for &mark in &marks {
                let text = format!("{letter}{mark}");
                system.shape(&text, &mut split);
                whole.clear();
                system.shape_direction(&text, right_to_left, &mut whole);
                assert_eq!(split, whole, "{:?}", text.escape_unicode().to_string());
            }
        }
    }

    /// The glyphs of `text` by cluster: their keys, offsets from the cluster's
    /// first glyph and advances, which is what placing them on the grid reads.
    fn by_cluster(system: &mut CoreTextSystem, text: &str) -> Vec<(u32, GlyphKey, i32, i32, i32)> {
        let mut out = Vec::new();
        system.shape(text, &mut out);
        let mut glyphs: Vec<_> = out
            .iter()
            .map(|g| {
                let first = out.iter().find(|f| f.cluster == g.cluster).unwrap();
                let dx = ((g.x - first.x) * 64.0).round() as i32;
                let advance = (g.advance * 64.0).round() as i32;
                (g.cluster, g.key, dx, (g.y * 64.0).round() as i32, advance)
            })
            .collect();
        glyphs.sort_by_key(|g| (g.0, g.1));
        glyphs
    }

    #[test]
    fn runs_of_one_direction_shape_as_if_each_were_alone() {
        let mut system = menlo(2.0);
        let rows = [
            "\u{644}a\u{627}",
            "\u{628}a\u{628}a\u{628}",
            "\u{633}\u{644}\u{627}\u{645} x \u{639}\u{644}\u{64a}\u{643}\u{645} (1) \u{645}\u{631}\u{62d}\u{628}\u{627}",
            "\u{5e9}\u{5c1}\u{5b8}a\u{5d1}\u{5bc}\u{5b7}\u{5e8} 2024 \u{5e2}\u{5d5}\u{5dc}\u{5b8}\u{5dd}",
            "f(\u{5d0}) <\u{5d1}> [\u{5d2}] fi\u{5d3}fl",
            "\u{5d0}\u{5b4}a\u{5b4}\u{5d1}",
            // What a shaper can build across graphemes, on both sides of a
            // run of the other direction.
            "\u{1f1ef}\u{5d0}\u{1f1f5}",
            "\u{1f1ef}\u{1f1f5}\u{5d0}\u{1f1fa}\u{1f1f8}",
            "\u{1f468}\u{200d}\u{5d0}\u{200d}\u{1f469}",
            "\u{1f468}\u{5d0}\u{200d}\u{1f469}",
            "\u{1100}\u{5d0}\u{1161}\u{11a8}",
            "\u{915}\u{94d}\u{5d0}\u{937}",
            "\u{915}\u{5d0}\u{94d}\u{937}",
            "f\u{5d0}i \u{644}a\u{627} \u{5d0}\u{1f1ef}\u{5d1}\u{1f1f5}",
            "\u{e01}\u{5d0}\u{e49}\u{e33}",
        ];
        for row in rows {
            let mut want = Vec::new();
            let mut start = 0;
            let mut current = None;
            for (byte, grapheme) in row.grapheme_indices(true) {
                let direction = grapheme_direction(grapheme).or(current).unwrap_or(false);
                if current.is_some_and(|c| c != direction) {
                    want.extend(
                        by_cluster(&mut system, &row[start..byte])
                            .into_iter()
                            .map(|(c, k, x, y, a)| (c + start as u32, k, x, y, a)),
                    );
                    start = byte;
                }
                current = Some(direction);
            }
            want.extend(
                by_cluster(&mut system, &row[start..])
                    .into_iter()
                    .map(|(c, k, x, y, a)| (c + start as u32, k, x, y, a)),
            );
            want.sort_by_key(|g| (g.0, g.1));
            system.lines = 0;
            assert_eq!(by_cluster(&mut system, row), want, "{row:?}");
            assert!(system.lines <= 2, "joined, not apart: {row:?}");
        }
    }

    #[test]
    fn a_row_whose_separator_draws_is_shaped_with_its_runs_apart() {
        let mut system = menlo(2.0);
        let row = "\u{644}a\u{627} \u{5d0}\u{5b8}b(\u{5d1})";
        let joined = by_cluster(&mut system, row);
        system.separator_draws = true;
        system.lines = 0;
        let apart = by_cluster(&mut system, row);
        let mut runs = Vec::new();
        direction::split(row, &mut runs);
        assert_eq!(system.lines, 2 + runs.len(), "joined, then each run alone");
        assert_eq!(apart, joined, "the same glyphs as each run alone");
    }

    #[test]
    fn a_row_makes_at_most_two_lines_however_often_it_changes_direction() {
        let mut system = menlo(1.0);
        let mut out = Vec::new();
        for unit in ["\u{5d0}a", "\u{628}a", "\u{5d0}\u{5b8}(", "a \u{5d0} "] {
            let row = unit.repeat(200);
            system.lines = 0;
            system.shape(&row, &mut out);
            assert_eq!(system.lines, 2, "{unit:?}");
            let starts: Vec<_> = row.grapheme_indices(true).map(|(b, _)| b as u32).collect();
            let mut drawn: Vec<_> = out
                .iter()
                .map(|g| {
                    assert!((g.cluster as usize) < row.len(), "no separator draws");
                    starts.partition_point(|&s| s <= g.cluster) - 1
                })
                .collect();
            drawn.sort_unstable();
            drawn.dedup();
            assert_eq!(drawn.len(), starts.len(), "every grapheme draws: {unit:?}");
        }
    }

    #[test]
    fn a_prepended_character_keeps_a_mirrored_form_out_of_level_1() {
        let mut system = menlo(1.0);
        let mut out = Vec::new();
        let want = key_of(&mut system, '(');
        let mut count = 0;
        for c in prepended() {
            count += 1;
            let text = format!("{c}(");
            system.shape(&text, &mut out);
            let glyph = out.iter().find(|g| g.cluster == c.len_utf8() as u32);
            let glyph = glyph.unwrap_or_else(|| panic!("{text:?}: {out:?}"));
            assert_eq!(glyph.key, want, "{:?}", text.escape_unicode().to_string());
        }
        assert!(count > 20, "{count}");
    }

    #[test]
    fn punctuation_with_no_mirrored_form_keeps_its_glyph_in_a_right_to_left_run() {
        let mut system = menlo(1.0);
        let mut out = Vec::new();
        for punctuation in (' '..='~').filter(|&c| grapheme_direction(&c.to_string()).is_none()) {
            let want = key_of(&mut system, punctuation);
            for letter in ['\u{5d0}', '\u{633}'] {
                let text = format!("{letter}{punctuation}{letter}");
                system.shape(&text, &mut out);
                let glyph = out.iter().find(|g| g.cluster == letter.len_utf8() as u32);
                let glyph = glyph.unwrap_or_else(|| panic!("{text:?}: {out:?}"));
                assert_eq!(glyph.key, want, "{text:?}");
            }
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
