//! The built-in themes (ADR 0027, ADR 0089): each a palette of named colours, the faces mapped
//! to those names, a float's panel style and the terms of its text gamma. Also the measures
//! the contrast tests check them by.
//!
//! Must not depend on: `atlas`, `grid`, `rows`, `scene`, `shape`, `text` or the GPU. It reads
//! the faces, and the renderer reads it.

#![allow(
    dead_code,
    reason = "a prototype: the scene takes the theme once M2-17 and M2-20 merge"
)]

use crate::pane::{Face, PANEL, PanelStyle};

/// Whether a theme is drawn on a dark background or a light one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dark,
    Light,
}

/// A colour as a theme's source writes it: sRGB, and an alpha that is straight and linear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Color {
    rgb: u32,
    alpha: f32,
}

/// An opaque colour, `0xRRGGBB` in sRGB.
const fn rgb(rgb: u32) -> Color {
    Color { rgb, alpha: 1.0 }
}

/// A colour at `alpha`, in linear light.
const fn rgba(rgb: u32, alpha: f32) -> Color {
    Color { rgb, alpha }
}

impl Color {
    /// In linear RGB, as the GPU blends.
    fn linear(self) -> [f32; 4] {
        let channel = |shift: u32| decode(((self.rgb >> shift) & 0xff) as f32 / 255.0);
        [channel(16), channel(8), channel(0), self.alpha]
    }
}

/// The names of a palette's colours. A face map refers to these, and each theme's palette
/// gives every one a colour (ADR 0027's two layers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Name {
    BgMain,
    BgFloat,
    BgKey,
    BgMode,
    BgRegion,
    BgEscape,
    FgMain,
    FgPrompt,
    FgKey,
    Escape,
    Warning,
    FieldOutline,
    Hairline,
    Shadow,
}

/// What a theme is made of, before it is resolved into linear colours.
struct Source {
    name: &'static str,
    kind: Kind,
    palette: fn(Name) -> Color,
    faces: fn(Face) -> (Name, Name),
    text: TextGamma,
}

/// How a theme corrects text's coverage (ADR 0089): `strength` 0 blends coverage in linear
/// light as it is, and 1 gives what blending in sRGB, as the OS draws text, gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextGamma {
    pub(crate) strength: f32,
}

impl TextGamma {
    /// Text as the OS draws it.
    pub(crate) const NATIVE: Self = Self { strength: 1.0 };
}

/// A theme, resolved: every face's colours in linear RGB.
pub struct Theme {
    name: &'static str,
    kind: Kind,
    foreground: Vec<[f32; 4]>,
    background: Vec<[f32; 4]>,
    panel: PanelStyle,
    text: TextGamma,
}

impl Theme {
    /// What the theme is called, as `load-theme` offers it.
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The colour of `face`'s coverage glyphs.
    pub(crate) fn foreground(&self, face: Face) -> [f32; 4] {
        self.foreground[index(face)]
    }

    /// The colour of `face`'s rectangles.
    pub(crate) fn background(&self, face: Face) -> [f32; 4] {
        self.background[index(face)]
    }

    pub(crate) fn panel(&self) -> &PanelStyle {
        &self.panel
    }

    pub(crate) fn text(&self) -> TextGamma {
        self.text
    }

    fn resolve(source: &Source) -> Self {
        let colors = |pick: fn((Name, Name)) -> Name| {
            FACES
                .iter()
                .map(|&face| (source.palette)(pick((source.faces)(face))).linear())
                .collect()
        };
        let shadow = (source.palette)(Name::Shadow).linear();
        Self {
            name: source.name,
            kind: source.kind,
            foreground: colors(|(fg, _)| fg),
            background: colors(|(_, bg)| bg),
            panel: PanelStyle { shadow, ..PANEL },
            text: source.text,
        }
    }
}

/// Every face, in the order of its variant, so that a theme keeps a table of them.
const FACES: [Face; 13] = [
    Face::Text,
    Face::Escape,
    Face::Boxed,
    Face::Cursor,
    Face::Modeline,
    Face::Warning,
    Face::Preedit,
    Face::Float,
    Face::FloatBorder,
    Face::Prompt,
    Face::Field,
    Face::Key,
    Face::Region,
];

/// Where `face` is in [`FACES`]. A match with no wildcard, so that a new face does not
/// compile until it is placed.
const fn index(face: Face) -> usize {
    match face {
        Face::Text => 0,
        Face::Escape => 1,
        Face::Boxed => 2,
        Face::Cursor => 3,
        Face::Modeline => 4,
        Face::Warning => 5,
        Face::Preedit => 6,
        Face::Float => 7,
        Face::FloatBorder => 8,
        Face::Prompt => 9,
        Face::Field => 10,
        Face::Key => 11,
        Face::Region => 12,
    }
}

/// The face map the themes share: each face's foreground and background, by name.
fn faces(face: Face) -> (Name, Name) {
    use Name::*;
    match face {
        Face::Text => (FgMain, BgMain),
        Face::Escape => (Escape, BgMain),
        Face::Boxed => (Escape, BgEscape),
        // The glyphs on the cursor take the background's colour, on a box of the text's.
        Face::Cursor => (BgMain, FgMain),
        Face::Modeline => (FgMain, BgMode),
        Face::Warning => (Warning, BgMain),
        Face::Preedit => (FgMain, FgMain),
        Face::Float => (FgMain, BgFloat),
        Face::FloatBorder => (Hairline, Hairline),
        Face::Prompt => (FgPrompt, BgFloat),
        Face::Field => (FieldOutline, BgMain),
        Face::Key => (FgKey, BgKey),
        Face::Region => (FgMain, BgRegion),
    }
}

/// The M2 steps' dark grey, with the float's colours as ADR 0075 picked them.
fn dusk(name: Name) -> Color {
    use Name::*;
    match name {
        BgMain => rgb(0x3f3f4b),
        BgFloat => rgb(0x474959),
        BgKey => rgb(0x555769),
        BgMode => rgb(0x6f6f7e),
        BgRegion => rgb(0x45597c),
        BgEscape => rgb(0x81593f),
        FgMain => rgb(0xf3f3f6),
        FgPrompt => rgb(0xbfc7df),
        FgKey => rgb(0xdee3f1),
        Escape => rgb(0xedb37c),
        Warning => rgb(0xffdd6f),
        FieldOutline => rgb(0x6f86bd),
        Hairline => rgba(0xffffff, 0.035),
        Shadow => rgba(0x08090e, 0.55),
    }
}

const SOURCES: [Source; 1] = [Source {
    name: "dusk",
    kind: Kind::Dark,
    palette: dusk,
    faces,
    text: TextGamma::NATIVE,
}];

/// The built-in themes, the default first, in linear colours. The renderer makes them once and
/// keeps them: there is no global state (ADR 0038).
pub fn themes() -> Vec<Theme> {
    SOURCES.iter().map(Theme::resolve).collect()
}

/// An sRGB value in linear light.
fn decode(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// A linear value in sRGB.
fn encode(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// WCAG's relative luminance of a linear colour.
pub(crate) fn luminance(color: [f32; 4]) -> f32 {
    0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
}

/// The coverage that, blended in linear light, gives what `coverage` blended in sRGB gives,
/// for text of luminance `ink` on a background of luminance `paper`, at `gamma`'s strength.
/// The text shader does the same (`text.wgsl`).
pub(crate) fn corrected_coverage(coverage: f32, ink: f32, paper: f32, gamma: TextGamma) -> f32 {
    if (ink - paper).abs() < 1e-3 {
        return coverage;
    }
    let native = decode(coverage * encode(ink) + (1.0 - coverage) * encode(paper));
    let wanted = ((native - paper) / (ink - paper)).clamp(0.0, 1.0);
    coverage + (wanted - coverage) * gamma.strength
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG's contrast ratio of two linear colours.
    fn contrast(a: [f32; 4], b: [f32; 4]) -> f32 {
        let (a, b) = (luminance(a) + 0.05, luminance(b) + 0.05);
        a.max(b) / a.min(b)
    }

    /// `color` laid over the opaque `under`, as the GPU blends it.
    fn over(color: [f32; 4], under: [f32; 4]) -> [f32; 4] {
        let a = color[3];
        let mix = |i: usize| color[i] * a + under[i] * (1.0 - a);
        [mix(0), mix(1), mix(2), 1.0]
    }

    /// OKLab's lightness, a and b of a linear colour.
    fn oklab(c: [f32; 4]) -> [f32; 3] {
        let l = (0.412_221_46 * c[0] + 0.536_332_55 * c[1] + 0.051_445_995 * c[2]).cbrt();
        let m = (0.211_903_5 * c[0] + 0.680_699_5 * c[1] + 0.107_396_96 * c[2]).cbrt();
        let s = (0.088_302_46 * c[0] + 0.281_718_85 * c[1] + 0.629_978_7 * c[2]).cbrt();
        [
            0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        ]
    }

    fn distance(a: [f32; 4], b: [f32; 4]) -> f32 {
        let (a, b) = (oklab(a), oklab(b));
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    /// What each face's foreground must reach on which background (ADR 0089). A match with no
    /// wildcard, so that a new face does not compile until it has its pairs.
    fn pairs(face: Face) -> &'static [(Face, f32)] {
        const READ: f32 = 7.0;
        const LABEL: f32 = 4.5;
        match face {
            Face::Text => &[(Face::Text, READ), (Face::Region, READ)],
            Face::Cursor => &[(Face::Cursor, READ)],
            Face::Modeline => &[(Face::Modeline, READ)],
            Face::Float => &[(Face::Float, READ)],
            Face::Warning => &[(Face::Text, READ), (Face::Modeline, LABEL)],
            Face::Escape => &[(Face::Text, LABEL)],
            Face::Boxed => &[(Face::Boxed, LABEL)],
            Face::Prompt => &[(Face::Float, LABEL)],
            Face::Key => &[(Face::Key, LABEL)],
            // Drawn as fills or lines, checked as such below.
            Face::Preedit | Face::FloatBorder | Face::Field | Face::Region => &[],
        }
    }

    #[test]
    fn faces_are_in_the_order_of_their_index() {
        for (i, &face) in FACES.iter().enumerate() {
            assert_eq!(index(face), i, "{face:?}");
        }
    }

    #[test]
    fn dusk_keeps_the_colours_of_the_m2_steps() {
        let themes = themes();
        let dusk = &themes[0];
        assert_eq!(dusk.name(), "dusk");
        for face in FACES {
            for (got, want) in [
                (dusk.foreground(face), face.foreground()),
                (dusk.background(face), face.background()),
            ] {
                let off = got.iter().zip(want).map(|(a, b)| (a - b).abs());
                assert!(
                    off.fold(0.0, f32::max) < 0.004,
                    "{face:?}: {got:?}, {want:?}"
                );
            }
        }
    }

    /// Every pair below its floor in every theme, as `theme: face on behind (ratio < floor)`.
    fn short_pairs() -> Vec<String> {
        let mut short = Vec::new();
        for theme in &themes() {
            for face in FACES {
                for &(behind, floor) in pairs(face) {
                    let ink = over(theme.foreground(face), theme.background(behind));
                    let ratio = contrast(ink, theme.background(behind));
                    if ratio < floor {
                        short.push(format!(
                            "{}: {face:?} on {behind:?} ({ratio:.2} < {floor})",
                            theme.name()
                        ));
                    }
                }
            }
        }
        short
    }

    #[test]
    fn dusk_misses_the_floors_where_the_author_has_yet_to_pick() {
        // The prototype's record of where today's colours fall short. Picking the colours
        // empties this, and the test then asserts no pair is short.
        assert_eq!(
            short_pairs(),
            [
                "dusk: Text on Region (6.38 < 7)",
                "dusk: Boxed on Boxed (3.30 < 4.5)",
                "dusk: Modeline on Modeline (4.46 < 7)",
                "dusk: Warning on Modeline (3.72 < 4.5)",
            ]
        );
    }

    #[test]
    fn fills_stand_apart_from_the_background() {
        for theme in &themes() {
            let paper = theme.background(Face::Text);
            for face in [
                Face::Region,
                Face::Modeline,
                Face::Float,
                Face::Key,
                Face::Boxed,
            ] {
                let step = (oklab(theme.background(face))[0] - oklab(paper)[0]).abs();
                assert!(step >= 0.02, "{}: {face:?} {step:.3}", theme.name());
                assert!(distance(theme.background(face), paper) >= 0.02);
            }
        }
    }

    #[test]
    fn corrected_coverage_keeps_its_ends_and_follows_the_ink() {
        let full = TextGamma { strength: 1.0 };
        let (light, dark) = (0.80, 0.03);
        for (ink, paper) in [(light, dark), (dark, light)] {
            assert_eq!(corrected_coverage(0.0, ink, paper, full), 0.0);
            assert!((corrected_coverage(1.0, ink, paper, full) - 1.0).abs() < 1e-5);
        }
        // Light text on dark comes out lighter than linear blending gives, dark text on light
        // heavier.
        assert!(corrected_coverage(0.5, light, dark, full) < 0.4);
        assert!(corrected_coverage(0.5, dark, light, full) > 0.6);
        let none = TextGamma { strength: 0.0 };
        assert_eq!(corrected_coverage(0.5, light, dark, none), 0.5);
        assert_eq!(corrected_coverage(0.5, 0.4, 0.4, full), 0.5);
    }

    #[test]
    fn corrected_coverage_gives_what_blending_in_srgb_gives() {
        let full = TextGamma { strength: 1.0 };
        let (ink, paper) = (0.80, 0.03);
        for step in 0..=10 {
            let coverage = step as f32 / 10.0;
            let a = corrected_coverage(coverage, ink, paper, full);
            let linear = a * ink + (1.0 - a) * paper;
            let srgb = coverage * encode(ink) + (1.0 - coverage) * encode(paper);
            assert!((encode(linear) - srgb).abs() < 1e-4, "{coverage}");
        }
    }
}
