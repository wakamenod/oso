//! The built-in themes (ADR 0027, ADR 0089): each a palette of named colours, the faces mapped
//! to those names, a float's panel style and the strength of its text gamma. Also the measures
//! the contrast tests check them by.
//!
//! Must not depend on: `atlas`, `grid`, `rows`, `scene`, `shape`, `text` or the GPU. It reads
//! the faces, and the renderer reads it.

use crate::pane::Face;

/// Whether a theme is drawn on a dark background or a light one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dark,
    Light,
}

/// A colour as a theme's source writes it: sRGB, and an alpha that is straight and linear.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Color {
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
    BgMatch,
    BgCurrent,
    BgSelected,
    FgMain,
    FgPrompt,
    FgKey,
    /// A question's words, apart from every colour buffer text is drawn in.
    FgQuestion,
    Escape,
    Warning,
    FieldOutline,
    /// A completion's matched characters.
    Gold,
    /// What is said of a candidate.
    Slate,
    /// A candidate's key binding.
    Azure,
    /// A completion's count and label.
    Teal,
    Hairline,
    Border,
    Shadow,
    /// Whitespace's marks: dim, to notice rather than read.
    FgWhitespace,
    /// Trailing whitespace's fill.
    BgTrailing,
    /// The current line's band.
    BgCurrentLine,
}

/// How a float's panel is drawn, beyond its faces' colours: sizes in points (the cell's height
/// over 16), and the shadow's and border's colours (ADR 0075, ADR 0076). A light theme wants a
/// lighter, smaller shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PanelStyle {
    pub(crate) radius: f32,
    /// Outside the fill, in [`Face::FloatBorder`].
    pub(crate) hairline: f32,
    /// Linear RGB and its strength, straight alpha.
    pub(crate) shadow: [f32; 4],
    pub(crate) shadow_down: f32,
    pub(crate) shadow_sigma: f32,
    /// How far a field reaches past its cells, across and up and down.
    pub(crate) field_reach: [f32; 2],
    pub(crate) field_radius: f32,
    pub(crate) field_outline: f32,
    /// How far a key's chip stays inside its cells, across and up and down.
    pub(crate) key_inset: [f32; 2],
    pub(crate) key_radius: f32,
    /// How far the selected candidate's box reaches past its row's cells, across.
    pub(crate) selected_reach: f32,
    pub(crate) selected_radius: f32,
    /// The border of a panel that has one, such as a completion's (ADR 0076), in place of the
    /// hairline: its width, and its colour in linear RGB.
    pub(crate) border_width: f32,
    pub(crate) border: [f32; 4],
}

/// The panel's sizes, which every theme shares so far. Its colours come from the palette.
const SIZES: PanelStyle = PanelStyle {
    radius: 8.0,
    hairline: 1.0,
    shadow: [0.0; 4],
    shadow_down: 10.0,
    shadow_sigma: 14.0,
    field_reach: [6.0, 4.0],
    field_radius: 5.0,
    field_outline: 1.0,
    key_inset: [1.0, 2.0],
    key_radius: 4.0,
    selected_reach: 6.0,
    selected_radius: 4.0,
    border_width: 5.0,
    border: [0.0; 4],
};

/// How a theme corrects text's coverage (ADR 0089): `strength` 0 blends coverage in linear
/// light as it is, and 1 gives what blending in sRGB, as the OS draws text, gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextGamma {
    pub(crate) strength: f32,
}

/// What a theme is made of, before it is resolved into linear colours.
pub(crate) struct Source {
    name: &'static str,
    kind: Kind,
    palette: fn(Name) -> Color,
    faces: fn(Face) -> (Name, Name),
    text: TextGamma,
}

/// A theme, resolved: every face's colours in linear RGB. It is small and holds nothing on the
/// heap, so the renderer keeps a copy of the current one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    name: &'static str,
    kind: Kind,
    foreground: [[f32; 4]; FACES.len()],
    background: [[f32; 4]; FACES.len()],
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

    /// The theme with its text corrected by `text`, for tests.
    #[cfg(test)]
    pub(crate) fn with_text(self, text: TextGamma) -> Self {
        Self { text, ..self }
    }

    /// The theme with every face's colours swapped, foreground for background, for tests.
    #[cfg(test)]
    pub(crate) fn swapped(self) -> Self {
        Self {
            foreground: self.background,
            background: self.foreground,
            ..self
        }
    }

    pub(crate) fn resolve(source: &Source) -> Self {
        let colors = |pick: fn((Name, Name)) -> Name| {
            FACES.map(|face| (source.palette)(pick((source.faces)(face))).linear())
        };
        let palette = |name| (source.palette)(name).linear();
        Self {
            name: source.name,
            kind: source.kind,
            foreground: colors(|(fg, _)| fg),
            background: colors(|(_, bg)| bg),
            panel: PanelStyle {
                shadow: palette(Name::Shadow),
                border: palette(Name::Border),
                ..SIZES
            },
            text: source.text,
        }
    }
}

/// Every face, in the order of [`index`], so that a theme keeps a table of them.
const FACES: [Face; 24] = [
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
    Face::Question,
    Face::Field,
    Face::Key,
    Face::Region,
    Face::CompletionSelected,
    Face::CompletionMatch,
    Face::CompletionAnnotation,
    Face::CompletionBinding,
    Face::CompletionPrompt,
    Face::SearchMatch,
    Face::SearchCurrent,
    Face::Whitespace,
    Face::TrailingWhitespace,
    Face::CurrentLine,
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
        Face::Question => 10,
        Face::Field => 11,
        Face::Key => 12,
        Face::Region => 13,
        Face::CompletionSelected => 14,
        Face::CompletionMatch => 15,
        Face::CompletionAnnotation => 16,
        Face::CompletionBinding => 17,
        Face::CompletionPrompt => 18,
        Face::SearchMatch => 19,
        Face::SearchCurrent => 20,
        Face::Whitespace => 21,
        Face::TrailingWhitespace => 22,
        Face::CurrentLine => 23,
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
        Face::Question => (FgQuestion, BgFloat),
        Face::Field => (FieldOutline, BgMain),
        Face::Key => (FgKey, BgKey),
        Face::Region => (FgMain, BgRegion),
        // A completion's float is filled with the text's background (ADR 0076).
        Face::CompletionSelected => (FgMain, BgSelected),
        Face::CompletionMatch => (Gold, BgMain),
        Face::CompletionAnnotation => (Slate, BgMain),
        Face::CompletionBinding => (Azure, BgMain),
        Face::CompletionPrompt => (Teal, BgMain),
        Face::SearchMatch => (FgMain, BgMatch),
        Face::SearchCurrent => (FgMain, BgCurrent),
        Face::Whitespace => (FgWhitespace, BgMain),
        Face::TrailingWhitespace => (FgMain, BgTrailing),
        Face::CurrentLine => (FgMain, BgCurrentLine),
    }
}

/// Near-black, with a high-contrast foreground: the default.
fn dark(name: Name) -> Color {
    use Name::*;
    match name {
        BgMain => rgb(0x0d0e12),
        BgFloat => rgb(0x181a22),
        BgKey => rgb(0x2b2e3b),
        BgMode => rgb(0x262938),
        BgRegion => rgb(0x26365c),
        BgEscape => rgb(0x3a2718),
        BgMatch => rgb(0x3a2e10),
        // The current match in teal, apart from the others' amber by hue.
        BgCurrent => rgb(0x00566a),
        BgSelected => rgb(0x1e3366),
        FgMain => rgb(0xeceef4),
        FgPrompt => rgb(0xa6b0d0),
        FgKey => rgb(0xe2e6f2),
        FgQuestion => rgb(0xb8dcff),
        Escape => rgb(0xfab06e),
        Warning => rgb(0xf5c84c),
        FieldOutline => rgb(0x5a76c2),
        Gold => rgb(0xf7c96e),
        Slate => rgb(0x9fabd2),
        Azure => rgb(0x6cc4ff),
        Teal => rgb(0x5ad6ae),
        FgWhitespace => rgb(0x9aa0b8),
        BgTrailing => rgb(0x3a1b22),
        BgCurrentLine => rgb(0x181a22),
        Hairline => rgba(0xffffff, 0.05),
        Border => rgb(0x30344a),
        Shadow => rgba(0x000000, 0.6),
    }
}

/// Near-white, with a near-black foreground: `dark`'s pair.
fn light(name: Name) -> Color {
    use Name::*;
    match name {
        BgMain => rgb(0xfbfbf8),
        BgFloat => rgb(0xeff0f5),
        BgKey => rgb(0xdadeea),
        BgMode => rgb(0xd9dce8),
        BgRegion => rgb(0xc8d6f5),
        BgEscape => rgb(0xf7e2cf),
        BgMatch => rgb(0xf6e6b4),
        BgCurrent => rgb(0x8fdcd0),
        BgSelected => rgb(0xccdcff),
        FgMain => rgb(0x15161b),
        FgPrompt => rgb(0x46507a),
        FgKey => rgb(0x15161b),
        FgQuestion => rgb(0x1a3a8c),
        Escape => rgb(0x8a3f00),
        Warning => rgb(0x6e4600),
        FieldOutline => rgb(0x4a6cc4),
        Gold => rgb(0x623c00),
        Slate => rgb(0x46507a),
        Azure => rgb(0x0058b0),
        Teal => rgb(0x006b52),
        FgWhitespace => rgb(0x5f6577),
        BgTrailing => rgb(0xf5d6d6),
        BgCurrentLine => rgb(0xeeefe8),
        Hairline => rgba(0x000000, 0.08),
        Border => rgb(0xc2c6d4),
        Shadow => rgba(0x1a1f33, 0.18),
    }
}

/// The M2 steps' dark grey, with the float's colours as ADR 0075 and ADR 0076 picked them.
/// Where those missed a floor, the nearest colour that meets it, its hue kept (ADR 0089).
fn dusk(name: Name) -> Color {
    use Name::*;
    match name {
        BgMain => rgb(0x3f3f4b),
        BgFloat => rgb(0x474959),
        BgKey => rgb(0x555769),
        BgMode => rgb(0x51515f),
        BgRegion => rgb(0x3f5375),
        BgEscape => rgb(0x6b452b),
        BgMatch => rgb(0x613f10),
        BgCurrent => rgb(0x005c4c),
        BgSelected => rgb(0x233670),
        FgMain => rgb(0xf3f3f6),
        FgPrompt => rgb(0xbfc7df),
        FgKey => rgb(0xdee3f1),
        // A pale green: no buffer text is drawn in it.
        FgQuestion => rgb(0xc8f7c0),
        Escape => rgb(0xf4ba83),
        Warning => rgb(0xffdd6f),
        FieldOutline => rgb(0x6f86bd),
        Gold => rgb(0xffce7c),
        Slate => rgb(0xaeb8de),
        Azure => rgb(0x4fc3ff),
        Teal => rgb(0x5fd7af),
        FgWhitespace => rgb(0xa3a7bd),
        BgTrailing => rgb(0x5d3a46),
        BgCurrentLine => rgb(0x4a4a52),
        Hairline => rgba(0xffffff, 0.035),
        Border => rgb(0x505265),
        Shadow => rgba(0x08090e, 0.55),
    }
}

/// `name`'s theme, with text gamma off until its strength is picked beside the OS's text.
const fn source(name: &'static str, kind: Kind, palette: fn(Name) -> Color) -> Source {
    Source {
        name,
        kind,
        palette,
        faces,
        text: TextGamma { strength: 0.0 },
    }
}

/// The built-in themes, the default first (ADR 0089).
const SOURCES: [Source; 3] = [
    source("dark", Kind::Dark, dark),
    source("light", Kind::Light, light),
    source("dusk", Kind::Dark, dusk),
];

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
#[cfg(test)]
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
/// The text shader does the same (`text.wgsl`), and the tests compare the two.
#[cfg(test)]
pub(crate) fn corrected_coverage(coverage: f32, ink: f32, paper: f32, gamma: TextGamma) -> f32 {
    if (ink - paper).abs() < 1e-3 {
        return coverage;
    }
    let native = decode(coverage * encode(ink) + (1.0 - coverage) * encode(paper));
    let wanted = ((native - paper) / (ink - paper)).clamp(0.0, 1.0);
    coverage + (wanted - coverage) * gamma.strength
}

#[cfg(test)]
pub(crate) mod tests {
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
        // Marks to notice, not text to read (ADR 0097).
        const MARK: f32 = 3.0;
        const LIST: [Face; 2] = [Face::Text, Face::CompletionSelected];
        match face {
            Face::Text => &[
                (Face::Text, READ),
                (Face::CurrentLine, READ),
                (Face::Region, READ),
                (Face::SearchMatch, READ),
                (Face::SearchCurrent, READ),
                (Face::CompletionSelected, READ),
            ],
            Face::Cursor => &[(Face::Cursor, READ)],
            Face::Modeline => &[(Face::Modeline, READ)],
            Face::Float => &[(Face::Float, READ)],
            Face::Question => &[(Face::Float, READ)],
            Face::Warning => &[(Face::Text, READ), (Face::Modeline, LABEL)],
            Face::CompletionMatch => &[(LIST[0], READ), (LIST[1], READ)],
            // An escape's label sits wherever text can: on every fill behind buffer text, in a
            // modeline's name and on a float.
            Face::Escape => &[
                (Face::Text, LABEL),
                (Face::CurrentLine, LABEL),
                (Face::Region, LABEL),
                (Face::SearchMatch, LABEL),
                (Face::SearchCurrent, LABEL),
                (Face::CompletionSelected, LABEL),
                (Face::Modeline, LABEL),
                (Face::Float, LABEL),
            ],
            Face::Boxed => &[(Face::Boxed, LABEL)],
            Face::Prompt => &[(Face::Float, LABEL)],
            Face::Key => &[(Face::Key, LABEL)],
            Face::CompletionAnnotation | Face::CompletionBinding => {
                &[(LIST[0], LABEL), (LIST[1], LABEL)]
            }
            Face::CompletionPrompt => &[(Face::Text, LABEL)],
            // A mark sits on everything behind buffer text.
            Face::Whitespace => &[
                (Face::Text, MARK),
                (Face::CurrentLine, MARK),
                (Face::TrailingWhitespace, MARK),
                (Face::Region, MARK),
                (Face::SearchMatch, MARK),
                (Face::SearchCurrent, MARK),
            ],
            // Drawn as fills or lines, checked as such below.
            Face::Preedit
            | Face::FloatBorder
            | Face::Field
            | Face::Region
            | Face::CompletionSelected
            | Face::SearchMatch
            | Face::SearchCurrent
            | Face::TrailingWhitespace
            | Face::CurrentLine => &[],
        }
    }

    /// Every pair's ratio and floor in `theme`, as a Markdown table, for the samples.
    pub(crate) fn ratios(theme: &Theme) -> String {
        let mut table = String::from("| Face | On | Ratio | Floor |\n|---|---|---|---|\n");
        for face in FACES {
            for &(behind, floor) in pairs(face) {
                let ink = over(theme.foreground(face), theme.background(behind));
                let ratio = contrast(ink, theme.background(behind));
                let short = if ratio < floor { " **short**" } else { "" };
                table += &format!("| {face:?} | {behind:?} | {ratio:.2}{short} | {floor} |\n");
            }
        }
        let apart = |a: Face, b: Face| distance(theme.background(a), theme.background(b));
        table += &format!(
            "\nOKLab distance: SearchCurrent to SearchMatch {:.3}, SearchMatch to Region {:.3}; \
             Question to Text {:.3}\n",
            apart(Face::SearchCurrent, Face::SearchMatch),
            apart(Face::SearchMatch, Face::Region),
            distance(
                theme.foreground(Face::Question),
                theme.foreground(Face::Text)
            ),
        );
        table
    }

    #[test]
    fn faces_are_in_the_order_of_their_index() {
        for (i, &face) in FACES.iter().enumerate() {
            assert_eq!(index(face), i, "{face:?}");
        }
    }

    #[test]
    fn dusk_keeps_the_colours_of_the_m2_steps_that_meet_their_floors() {
        // The constants `pane.rs` drew with before the themes, in linear RGB. The fills of
        // boxes, the modeline, the region, the selected candidate and the current match, an
        // escape's label and a question's words, are the nearest colours that meet the floors
        // instead (ADR 0089).
        let (bg, fg) = ([0.05, 0.05, 0.07, 1.0], [0.90, 0.90, 0.92, 1.0]);
        let float = [0.063, 0.0666, 0.0999, 1.0];
        let before = [
            (Face::Text, fg, bg),
            (Face::Cursor, bg, fg),
            (Face::Warning, [1.0, 0.72, 0.16, 1.0], bg),
            (Face::Preedit, fg, fg),
            (Face::Float, fg, float),
            (
                Face::FloatBorder,
                [1.0, 1.0, 1.0, 0.035],
                [1.0, 1.0, 1.0, 0.035],
            ),
            (Face::Prompt, [0.52, 0.57, 0.74, 1.0], float),
            (Face::Field, [0.159, 0.2384, 0.5089, 1.0], bg),
            (
                Face::Key,
                [0.73, 0.77, 0.88, 1.0],
                [0.0908, 0.0953, 0.1413, 1.0],
            ),
            (Face::CompletionMatch, [1.0, 0.62, 0.20, 1.0], bg),
            (Face::CompletionAnnotation, [0.423, 0.479, 0.730, 1.0], bg),
            (Face::CompletionBinding, [0.078, 0.546, 1.0, 1.0], bg),
            (Face::CompletionPrompt, [0.114, 0.680, 0.429, 1.0], bg),
            (Face::SearchMatch, fg, [0.12, 0.05, 0.005, 1.0]),
        ];
        let themes = themes();
        let dusk = themes.iter().find(|theme| theme.name() == "dusk").unwrap();
        let close = |got: [f32; 4], want: [f32; 4]| {
            got.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.004)
        };
        for (face, fg, bg) in before {
            assert!(close(dusk.foreground(face), fg), "{face:?}'s foreground");
            assert!(close(dusk.background(face), bg), "{face:?}'s background");
        }
        assert!(close(dusk.panel().shadow, [0.0024, 0.0027, 0.0044, 0.55]));
        assert!(close(dusk.panel().border, [0.08, 0.085, 0.13, 1.0]));
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
    fn every_pair_meets_its_floor_in_every_theme() {
        assert_eq!(short_pairs(), Vec::<String>::new());
    }

    #[test]
    fn the_default_theme_is_dark_and_light_is_its_pair() {
        let names: Vec<_> = themes()
            .iter()
            .map(|theme| (theme.name(), theme.kind()))
            .collect();
        assert_eq!(
            names,
            [
                ("dark", Kind::Dark),
                ("light", Kind::Light),
                ("dusk", Kind::Dark)
            ]
        );
    }

    #[test]
    fn fills_stand_apart_from_what_they_sit_on() {
        for theme in &themes() {
            let behind_text = theme.background(Face::Text);
            let float = theme.background(Face::Float);
            for (face, behind) in [
                (Face::Region, behind_text),
                (Face::SearchMatch, behind_text),
                (Face::SearchCurrent, behind_text),
                (Face::CompletionSelected, behind_text),
                (Face::Modeline, behind_text),
                (Face::Float, behind_text),
                (Face::Boxed, behind_text),
                // A key's chip and an input field sit on a float's fill.
                (Face::Key, float),
                (Face::Field, float),
                (Face::CurrentLine, behind_text),
                (Face::TrailingWhitespace, behind_text),
            ] {
                let step = (oklab(theme.background(face))[0] - oklab(behind)[0]).abs();
                assert!(step >= 0.02, "{}: {face:?} {step:.3}", theme.name());
            }
        }
    }

    #[test]
    fn what_fills_a_line_stands_apart_from_the_current_lines_band() {
        for theme in &themes() {
            let band = theme.background(Face::CurrentLine);
            for face in [
                Face::Region,
                Face::SearchMatch,
                Face::SearchCurrent,
                Face::TrailingWhitespace,
            ] {
                let apart = distance(theme.background(face), band);
                assert!(apart >= 0.05, "{}: {face:?} {apart:.3}", theme.name());
            }
        }
    }

    #[test]
    fn a_whitespace_mark_is_in_no_colour_text_is_drawn_in() {
        for theme in &themes() {
            let mark = theme.foreground(Face::Whitespace);
            for face in [Face::Text, Face::Escape, Face::Boxed, Face::Warning] {
                let apart = distance(mark, theme.foreground(face));
                assert!(apart >= 0.05, "{}: {face:?} {apart:.3}", theme.name());
            }
        }
    }

    #[test]
    fn what_a_completion_matched_stands_apart_from_the_rest_of_its_text() {
        for theme in &themes() {
            let matched = theme.foreground(Face::CompletionMatch);
            let apart = distance(matched, theme.foreground(Face::Text));
            assert!(apart >= 0.05, "{}: {apart:.3}", theme.name());
        }
    }

    #[test]
    fn a_questions_words_are_in_no_colour_buffer_text_is_drawn_in() {
        for theme in &themes() {
            let question = theme.foreground(Face::Question);
            for face in [Face::Text, Face::Escape, Face::Boxed, Face::Warning] {
                let apart = distance(question, theme.foreground(face));
                assert!(apart >= 0.05, "{}: {face:?} {apart:.3}", theme.name());
            }
        }
    }

    /// OKLab's hue of a linear colour, in degrees.
    fn hue(color: [f32; 4]) -> f32 {
        let [_, a, b] = oklab(color);
        b.atan2(a).to_degrees()
    }

    /// Faces that must be told apart (ADR 0089). The two search fills were 0.15 apart in
    /// OKLab and still read as one, both amber, so they also differ by hue.
    #[test]
    fn faces_that_must_be_told_apart_are_told_apart() {
        for theme in &themes() {
            let fill = |face| theme.background(face);
            let name = theme.name();
            let current = fill(Face::SearchCurrent);
            let others = fill(Face::SearchMatch);
            let turn = (hue(current) - hue(others)).rem_euclid(360.0);
            assert!(
                turn.min(360.0 - turn) >= 90.0,
                "{name}: the search fills' hues"
            );
            for (a, b) in [
                (Face::SearchCurrent, Face::SearchMatch),
                (Face::SearchMatch, Face::Region),
                (Face::SearchCurrent, Face::Region),
            ] {
                let apart = distance(fill(a), fill(b));
                assert!(apart >= 0.09, "{name}: {a:?} and {b:?} {apart:.3}");
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
