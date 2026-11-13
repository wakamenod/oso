//! The built-in themes (ADR 0027, ADR 0089): each a palette of named colours, the faces mapped
//! to those names, a float's panel style and the strength of its text gamma. Also the measures
//! the contrast tests check them by.
//!
//! Must not depend on: `atlas`, `grid`, `rows`, `scene`, `shape`, `text` or the GPU. It reads
//! the faces, and the renderer reads it.

use crate::pane::Face;

mod palettes;

use palettes::{Name, dark, dusk, light};

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
        let [r, g, b] = self.bytes();
        let channel = |byte: u8| decode(f32::from(byte) / 255.0);
        [channel(r), channel(g), channel(b), self.alpha]
    }

    /// Its sRGB bytes, red first. The alpha is left out.
    fn bytes(self) -> [u8; 3] {
        let [_, r, g, b] = self.rgb.to_be_bytes();
        [r, g, b]
    }
}

/// A theme's colours as a terminal's engine is given them, to answer a program that asks
/// (ADR 0120): the text's foreground and background, the cursor box's colour and the 16
/// terminal faces', in sRGB bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalPalette {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub cursor: [u8; 3],
    pub ansi: [[u8; 3]; 16],
}

/// The terminal faces, colour 0 first.
const TERMINAL: [Face; 16] = [
    Face::TerminalColor0,
    Face::TerminalColor1,
    Face::TerminalColor2,
    Face::TerminalColor3,
    Face::TerminalColor4,
    Face::TerminalColor5,
    Face::TerminalColor6,
    Face::TerminalColor7,
    Face::TerminalColor8,
    Face::TerminalColor9,
    Face::TerminalColor10,
    Face::TerminalColor11,
    Face::TerminalColor12,
    Face::TerminalColor13,
    Face::TerminalColor14,
    Face::TerminalColor15,
];

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
    /// How far a menu key's chip reaches past its cells, across, which hold only the key.
    pub(crate) key_reach: f32,
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
    key_reach: 3.0,
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
    terminal: TerminalPalette,
}

impl Theme {
    /// What the theme is called, as `load-theme` offers it.
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Its colours as a terminal's engine is given them (ADR 0120).
    pub fn terminal_palette(&self) -> TerminalPalette {
        self.terminal
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
        let bytes = |face, pick: fn((Name, Name)) -> Name| {
            (source.palette)(pick((source.faces)(face))).bytes()
        };
        let terminal = TerminalPalette {
            foreground: bytes(Face::Text, |(fg, _)| fg),
            background: bytes(Face::Text, |(_, bg)| bg),
            cursor: bytes(Face::Cursor, |(_, bg)| bg),
            ansi: TERMINAL.map(|face| bytes(face, |(fg, _)| fg)),
        };
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
            terminal,
        }
    }
}

/// The fills a syntax face is drawn on, besides the background. On every other fill, and
/// under the flash, highlighted text keeps [`Face::Text`]'s colour, so a syntax face is checked
/// on these only (ADR 0111).
///
/// A review's bands (ADR 0122) follow: behind its rows only its own faces are drawn until step 9
/// of ADR 0122 highlights the sides, which checks the syntax faces on them too. Its refined
/// words' bands are not here, so text on them is drawn in [`Face::Text`]'s colour.
pub(crate) const SYNTAX_FILLS: [Face; 5] = [
    Face::CurrentLine,
    Face::DiffAdded,
    Face::DiffRemoved,
    Face::DiffFileHeader,
    Face::ReviewCommentPerson,
];

/// Every face, in the order of [`index`], so that a theme keeps a table of them.
const FACES: [Face; 74] = [
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
    Face::SecondaryCursor,
    Face::Whitespace,
    Face::TrailingWhitespace,
    Face::CurrentLine,
    Face::Flash,
    Face::LineNumber,
    Face::LineNumberCurrent,
    Face::WrapMark,
    Face::ModelineInactive,
    Face::Divider,
    Face::SyntaxComment,
    Face::SyntaxString,
    Face::SyntaxKeyword,
    Face::SyntaxType,
    Face::SyntaxFunction,
    Face::SyntaxConstant,
    Face::SyntaxProperty,
    Face::SyntaxMacro,
    Face::SyntaxHeading,
    Face::SyntaxLink,
    Face::SyntaxMarkup,
    Face::JumpLabel,
    Face::JumpLabelNext,
    Face::MenuKey,
    Face::MenuKeyStay,
    Face::MenuKeySwitch,
    Face::MenuTitle,
    Face::MenuNote,
    Face::DiffAdded,
    Face::DiffRemoved,
    Face::DiffRefineAdded,
    Face::DiffRefineRemoved,
    Face::DiffHunkHeader,
    Face::DiffFileHeader,
    Face::ReviewCommentPerson,
    Face::ReviewCommentAgent,
    Face::ReviewOutdated,
    Face::TerminalColor0,
    Face::TerminalColor1,
    Face::TerminalColor2,
    Face::TerminalColor3,
    Face::TerminalColor4,
    Face::TerminalColor5,
    Face::TerminalColor6,
    Face::TerminalColor7,
    Face::TerminalColor8,
    Face::TerminalColor9,
    Face::TerminalColor10,
    Face::TerminalColor11,
    Face::TerminalColor12,
    Face::TerminalColor13,
    Face::TerminalColor14,
    Face::TerminalColor15,
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
        Face::SecondaryCursor => 21,
        Face::Whitespace => 22,
        Face::TrailingWhitespace => 23,
        Face::CurrentLine => 24,
        Face::Flash => 25,
        Face::LineNumber => 26,
        Face::LineNumberCurrent => 27,
        Face::WrapMark => 28,
        Face::ModelineInactive => 29,
        Face::Divider => 30,
        Face::SyntaxComment => 31,
        Face::SyntaxString => 32,
        Face::SyntaxKeyword => 33,
        Face::SyntaxType => 34,
        Face::SyntaxFunction => 35,
        Face::SyntaxConstant => 36,
        Face::SyntaxProperty => 37,
        Face::SyntaxMacro => 38,
        Face::SyntaxHeading => 39,
        Face::SyntaxLink => 40,
        Face::SyntaxMarkup => 41,
        Face::JumpLabel => 42,
        Face::JumpLabelNext => 43,
        Face::MenuKey => 44,
        Face::MenuKeyStay => 45,
        Face::MenuKeySwitch => 46,
        Face::MenuTitle => 47,
        Face::MenuNote => 48,
        Face::DiffAdded => 49,
        Face::DiffRemoved => 50,
        Face::DiffRefineAdded => 51,
        Face::DiffRefineRemoved => 52,
        Face::DiffHunkHeader => 53,
        Face::DiffFileHeader => 54,
        Face::ReviewCommentPerson => 55,
        Face::ReviewCommentAgent => 56,
        Face::ReviewOutdated => 57,
        Face::TerminalColor0 => 58,
        Face::TerminalColor1 => 59,
        Face::TerminalColor2 => 60,
        Face::TerminalColor3 => 61,
        Face::TerminalColor4 => 62,
        Face::TerminalColor5 => 63,
        Face::TerminalColor6 => 64,
        Face::TerminalColor7 => 65,
        Face::TerminalColor8 => 66,
        Face::TerminalColor9 => 67,
        Face::TerminalColor10 => 68,
        Face::TerminalColor11 => 69,
        Face::TerminalColor12 => 70,
        Face::TerminalColor13 => 71,
        Face::TerminalColor14 => 72,
        Face::TerminalColor15 => 73,
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
        Face::Modeline => (FgMode, BgMode),
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
        // An outline in the prompt's dim colour, drawn as a fill (ADR 0091).
        Face::SecondaryCursor => (FgMain, FgPrompt),
        Face::Whitespace => (FgWhitespace, BgMain),
        Face::TrailingWhitespace => (FgMain, BgTrailing),
        Face::CurrentLine => (FgMain, BgCurrentLine),
        Face::Flash => (FgMain, BgFlash),
        Face::LineNumber => (FgLineNumber, BgMain),
        Face::LineNumberCurrent => (FgLineNumberCurrent, BgMain),
        Face::WrapMark => (FgWrapMark, BgMain),
        Face::ModelineInactive => (FgModeInactive, BgModeInactive),
        // A line, drawn as a fill.
        Face::Divider => (Divider, Divider),
        // A foreground only: the text's background is behind it (ADR 0111).
        Face::SyntaxComment => (SynComment, BgMain),
        Face::SyntaxString => (SynString, BgMain),
        Face::SyntaxKeyword => (SynKeyword, BgMain),
        Face::SyntaxType => (SynType, BgMain),
        Face::SyntaxFunction => (SynFunction, BgMain),
        Face::SyntaxConstant => (SynConstant, BgMain),
        // Fields are drawn as text.
        Face::SyntaxProperty => (FgMain, BgMain),
        Face::SyntaxMacro => (SynMacro, BgMain),
        Face::SyntaxHeading => (SynHeading, BgMain),
        Face::SyntaxLink => (SynLink, BgMain),
        Face::SyntaxMarkup => (SynMarkup, BgMain),
        // Drawn over the text, each in a fill of its own (ADR 0116).
        Face::JumpLabel => (FgJump, BgJump),
        Face::JumpLabelNext => (FgJumpNext, BgJumpNext),
        // A menu's float is a completion's panel, on the text's background (ADR 0117).
        Face::MenuKey => (FgMenuKey, BgMenuKey),
        Face::MenuKeyStay => (FgMenuKey, BgMenuStay),
        Face::MenuKeySwitch => (FgMenuKey, BgMenuSwitch),
        Face::MenuTitle => (Azure, BgMain),
        Face::MenuNote => (Slate, BgMain),
        // A review's rows (ADR 0122): each text face on its band, which the window fills behind
        // the rows in view. The refined words' bands are fills only, behind their line's text.
        Face::DiffAdded => (FgAdded, BgAdded),
        Face::DiffRemoved => (FgRemoved, BgRemoved),
        Face::DiffRefineAdded => (FgAdded, BgRefineAdded),
        Face::DiffRefineRemoved => (FgRemoved, BgRefineRemoved),
        Face::DiffHunkHeader => (FgHunkHeader, BgHeader),
        Face::DiffFileHeader => (FgFileHeader, BgHeader),
        Face::ReviewCommentPerson => (FgCommentPerson, BgCard),
        Face::ReviewCommentAgent => (FgCommentAgent, BgCard),
        Face::ReviewOutdated => (Slate, BgCard),
        // One colour each, on the text's background (ADR 0120).
        Face::TerminalColor0 => (Terminal(0), BgMain),
        Face::TerminalColor1 => (Terminal(1), BgMain),
        Face::TerminalColor2 => (Terminal(2), BgMain),
        Face::TerminalColor3 => (Terminal(3), BgMain),
        Face::TerminalColor4 => (Terminal(4), BgMain),
        Face::TerminalColor5 => (Terminal(5), BgMain),
        Face::TerminalColor6 => (Terminal(6), BgMain),
        Face::TerminalColor7 => (Terminal(7), BgMain),
        Face::TerminalColor8 => (Terminal(8), BgMain),
        Face::TerminalColor9 => (Terminal(9), BgMain),
        Face::TerminalColor10 => (Terminal(10), BgMain),
        Face::TerminalColor11 => (Terminal(11), BgMain),
        Face::TerminalColor12 => (Terminal(12), BgMain),
        Face::TerminalColor13 => (Terminal(13), BgMain),
        Face::TerminalColor14 => (Terminal(14), BgMain),
        Face::TerminalColor15 => (Terminal(15), BgMain),
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
pub(crate) fn decode(value: f32) -> f32 {
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

    /// Plain text is read on every fill a line of it sits on, and on a selected candidate.
    const TEXT: [(Face, f32); 10] = [
        (Face::Text, 7.0),
        // A refined word's band, and a comment's text on its card (ADR 0122).
        (Face::DiffRefineAdded, 7.0),
        (Face::DiffRefineRemoved, 7.0),
        (Face::ReviewCommentPerson, 7.0),
        (Face::CurrentLine, 7.0),
        (Face::Flash, 7.0),
        (Face::Region, 7.0),
        (Face::SearchMatch, 7.0),
        (Face::SearchCurrent, 7.0),
        (Face::CompletionSelected, 7.0),
    ];

    /// Highlighted text is read on the background, on [`SYNTAX_FILLS`], and on a float's
    /// background, where the preview of `switch-to-buffer` and the mark list draws it.
    const SYNTAX_PAIRS: [(Face, f32); 3] = [
        (Face::Text, 7.0),
        (SYNTAX_FILLS[0], 7.0),
        (Face::Float, 7.0),
    ];

    /// An escape's label sits wherever text can: on every fill behind buffer text, a review's
    /// bands among them, in a modeline's name and on a float.
    const ESCAPE_PAIRS: [(Face, f32); 16] = [
        (Face::Text, 4.5),
        (Face::CurrentLine, 4.5),
        (Face::Flash, 4.5),
        (Face::Region, 4.5),
        (Face::SearchMatch, 4.5),
        (Face::SearchCurrent, 4.5),
        (Face::CompletionSelected, 4.5),
        (Face::Modeline, 4.5),
        (Face::ModelineInactive, 4.5),
        (Face::Float, 4.5),
        (Face::DiffAdded, 4.5),
        (Face::DiffRemoved, 4.5),
        (Face::DiffRefineAdded, 4.5),
        (Face::DiffRefineRemoved, 4.5),
        (Face::DiffFileHeader, 4.5),
        (Face::ReviewCommentPerson, 4.5),
    ];

    /// A whitespace or wrap mark sits on everything behind buffer text (ADR 0097).
    const MARK_PAIRS: [(Face, f32); 13] = [
        (Face::Text, 3.0),
        (Face::CurrentLine, 3.0),
        (Face::TrailingWhitespace, 3.0),
        (Face::Flash, 3.0),
        (Face::Region, 3.0),
        (Face::SearchMatch, 3.0),
        (Face::SearchCurrent, 3.0),
        (Face::DiffAdded, 3.0),
        (Face::DiffRemoved, 3.0),
        (Face::DiffRefineAdded, 3.0),
        (Face::DiffRefineRemoved, 3.0),
        (Face::DiffFileHeader, 3.0),
        (Face::ReviewCommentPerson, 3.0),
    ];

    /// What each face's foreground must reach on which background (ADR 0089). A match with no
    /// wildcard, so that a new face does not compile until it has its pairs.
    #[expect(clippy::too_many_lines, reason = "one arm per face or group of faces")]
    fn pairs(face: Face) -> &'static [(Face, f32)] {
        const READ: f32 = 7.0;
        const LABEL: f32 = 4.5;
        // Marks to notice, not text to read (ADR 0097).
        const MARK: f32 = 3.0;
        const LIST: [Face; 2] = [Face::Text, Face::CompletionSelected];
        match face {
            Face::Text => &TEXT,
            Face::Cursor => &[(Face::Cursor, READ)],
            Face::Modeline => &[(Face::Modeline, READ)],
            Face::ModelineInactive => &[(Face::ModelineInactive, LABEL)],
            // A line to notice on the text's background (ADR 0112).
            Face::Divider => &[(Face::Text, MARK)],
            Face::Float => &[(Face::Float, READ)],
            Face::Question => &[(Face::Float, READ)],
            Face::Warning => &[
                (Face::Text, READ),
                (Face::Modeline, LABEL),
                (Face::ModelineInactive, LABEL),
            ],
            Face::CompletionMatch => &[(LIST[0], READ), (LIST[1], READ)],
            Face::Escape => &ESCAPE_PAIRS,
            Face::Boxed => &[(Face::Boxed, LABEL)],
            // A prompt's label, and the dim `Agent` on a modeline (ADR 0085, ADR 0112).
            Face::Prompt => &[
                (Face::Float, LABEL),
                (Face::Modeline, LABEL),
                (Face::ModelineInactive, LABEL),
            ],
            Face::Key => &[(Face::Key, LABEL)],
            Face::CompletionAnnotation | Face::CompletionBinding | Face::MenuNote => {
                &[(LIST[0], LABEL), (LIST[1], LABEL)]
            }
            Face::CompletionPrompt => &[(Face::Text, LABEL)],
            // The gutter has no band and no fill, and a menu's titles are on the text's background.
            Face::LineNumber | Face::MenuTitle => &[(Face::Text, LABEL)],
            Face::LineNumberCurrent => &[(Face::Text, READ)],
            Face::Whitespace | Face::WrapMark => &MARK_PAIRS,
            // Text that is read, on what is behind a line. Under the fills and the flash it is
            // drawn in `Text`'s colour (ADR 0111, the author's choice).
            Face::SyntaxComment
            | Face::SyntaxString
            | Face::SyntaxKeyword
            | Face::SyntaxType
            | Face::SyntaxFunction
            | Face::SyntaxConstant
            | Face::SyntaxProperty
            | Face::SyntaxMacro
            | Face::SyntaxHeading
            | Face::SyntaxLink
            | Face::SyntaxMarkup => &SYNTAX_PAIRS,
            // A jump's label is read on its own fill (ADR 0116). Nothing else is drawn on it, as
            // the text's glyphs under a label are not drawn: no escape or mark lists its fill.
            Face::JumpLabel => &[(Face::JumpLabel, READ)],
            Face::JumpLabelNext => &[(Face::JumpLabelNext, READ)],
            // A menu's keys on their chips (ADR 0117).
            Face::MenuKey => &[(Face::MenuKey, LABEL)],
            Face::MenuKeyStay => &[(Face::MenuKeyStay, LABEL)],
            Face::MenuKeySwitch => &[(Face::MenuKeySwitch, LABEL)],
            // A review's text on its bands (ADR 0122): the headers share one band, and the
            // comments another.
            Face::DiffAdded => &[(Face::DiffAdded, READ)],
            Face::DiffRemoved => &[(Face::DiffRemoved, READ)],
            Face::DiffHunkHeader | Face::DiffFileHeader => &[(Face::DiffFileHeader, READ)],
            Face::ReviewCommentPerson | Face::ReviewCommentAgent => {
                &[(Face::ReviewCommentPerson, READ)]
            }
            Face::ReviewOutdated => &[(Face::ReviewCommentPerson, LABEL), (Face::Text, LABEL)],
            // A terminal's colours are read on the text's background, as highlighted text is,
            // but its blacks, greys and whites, which programs draw on one another (ADR 0120).
            Face::TerminalColor1
            | Face::TerminalColor2
            | Face::TerminalColor3
            | Face::TerminalColor4
            | Face::TerminalColor5
            | Face::TerminalColor6
            | Face::TerminalColor9
            | Face::TerminalColor10
            | Face::TerminalColor11
            | Face::TerminalColor12
            | Face::TerminalColor13
            | Face::TerminalColor14 => &[(Face::Text, READ)],
            Face::TerminalColor0
            | Face::TerminalColor7
            | Face::TerminalColor8
            | Face::TerminalColor15 => &[],
            // Drawn as fills or lines, checked as such below.
            Face::Preedit
            | Face::FloatBorder
            | Face::Field
            | Face::Region
            | Face::CompletionSelected
            | Face::SearchMatch
            | Face::SearchCurrent
            | Face::SecondaryCursor
            | Face::TrailingWhitespace
            | Face::CurrentLine
            | Face::Flash
            | Face::DiffRefineAdded
            | Face::DiffRefineRemoved => &[],
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
        themes().iter().flat_map(short_pairs_of).collect()
    }

    /// Each pair of `theme` short of its floor.
    fn short_pairs_of(theme: &Theme) -> Vec<String> {
        let mut short = Vec::new();
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
                (Face::ModelineInactive, behind_text),
                (Face::Float, behind_text),
                (Face::Boxed, behind_text),
                // A key's chip and an input field sit on a float's fill.
                (Face::Key, float),
                (Face::Field, float),
                (Face::CurrentLine, behind_text),
                (Face::TrailingWhitespace, behind_text),
                (Face::Flash, behind_text),
                (Face::JumpLabel, behind_text),
                (Face::JumpLabelNext, behind_text),
                (Face::DiffAdded, behind_text),
                (Face::DiffRemoved, behind_text),
                (Face::DiffFileHeader, behind_text),
                (Face::ReviewCommentPerson, behind_text),
                (Face::DiffRefineAdded, theme.background(Face::DiffAdded)),
                (Face::DiffRefineRemoved, theme.background(Face::DiffRemoved)),
            ] {
                let step = (oklab(theme.background(face))[0] - oklab(behind)[0]).abs();
                assert!(step >= 0.02, "{}: {face:?} {step:.3}", theme.name());
            }
        }
    }

    /// A menu's key chips are marks on its panel, the text's background, and on the box that
    /// marks the default head's row, at 3:1, and the three kinds are told apart by their fills
    /// (ADR 0117).
    #[test]
    fn a_menus_key_chips_stand_out_on_its_panel_and_apart() {
        let chips = [Face::MenuKey, Face::MenuKeyStay, Face::MenuKeySwitch];
        for theme in &themes() {
            let name = theme.name();
            for under in [Face::Text, Face::CompletionSelected] {
                for chip in chips {
                    let ratio = contrast(theme.background(chip), theme.background(under));
                    assert!(ratio >= 3.0, "{name}: {chip:?} on {under:?} {ratio:.2}");
                }
            }
            for (i, a) in chips.iter().enumerate() {
                for b in &chips[i + 1..] {
                    let apart = distance(theme.background(*a), theme.background(*b));
                    assert!(apart >= 0.09, "{name}: {a:?} and {b:?} {apart:.3}");
                }
            }
        }
    }

    /// A secondary cursor's outline stands out on the text's background, as WCAG asks of
    /// what is no text, at 3:1, and less than the cursor box, so the primary is the one that
    /// stands out (ADR 0091).
    #[test]
    fn a_secondary_cursor_is_seen_and_dimmer_than_the_primary() {
        for theme in &themes() {
            let behind = theme.background(Face::Text);
            let outline = contrast(theme.background(Face::SecondaryCursor), behind);
            let primary = contrast(theme.background(Face::Cursor), behind);
            assert!(outline >= 3.0, "{}: {outline:.2}", theme.name());
            assert!(
                outline < primary,
                "{}: {outline:.2} {primary:.2}",
                theme.name()
            );
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
                Face::Flash,
                Face::JumpLabel,
                Face::JumpLabelNext,
            ] {
                let apart = distance(theme.background(face), band);
                assert!(apart >= 0.05, "{}: {face:?} {apart:.3}", theme.name());
            }
        }
    }

    /// The flash is no fill the cursor's row can have for another reason, so it never reads as
    /// a region or a match there.
    #[test]
    fn the_flash_stands_apart_from_every_other_fill() {
        for theme in &themes() {
            let flash = theme.background(Face::Flash);
            for face in [
                Face::Region,
                Face::SearchMatch,
                Face::SearchCurrent,
                Face::TrailingWhitespace,
                Face::CompletionSelected,
                Face::JumpLabel,
                Face::JumpLabelNext,
            ] {
                let apart = distance(theme.background(face), flash);
                assert!(apart >= 0.05, "{}: {face:?} {apart:.3}", theme.name());
            }
        }
    }

    #[test]
    fn the_current_lines_number_stands_apart_from_the_others() {
        for theme in &themes() {
            let current = theme.foreground(Face::LineNumberCurrent);
            let apart = distance(current, theme.foreground(Face::LineNumber));
            assert!(apart >= 0.05, "{}: {apart:.3}", theme.name());
        }
    }

    #[test]
    fn a_wrap_mark_is_in_no_colour_text_or_a_whitespace_mark_is_drawn_in() {
        for theme in &themes() {
            let mark = theme.foreground(Face::WrapMark);
            let text = [
                Face::Text,
                Face::Preedit,
                Face::Escape,
                Face::Boxed,
                Face::Warning,
                Face::Whitespace,
            ];
            for face in text {
                let apart = distance(mark, theme.foreground(face));
                assert!(apart >= 0.05, "{}: {face:?} {apart:.3}", theme.name());
            }
        }
    }

    #[test]
    fn a_whitespace_mark_is_in_no_colour_text_is_drawn_in() {
        for theme in &themes() {
            let mark = theme.foreground(Face::Whitespace);
            let text = [
                Face::Text,
                Face::Preedit,
                Face::Escape,
                Face::Boxed,
                Face::Warning,
            ];
            for face in text {
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

    /// The syntax faces, in the order of [`index`].
    pub(crate) const SYNTAX: [Face; 11] = [
        Face::SyntaxComment,
        Face::SyntaxString,
        Face::SyntaxKeyword,
        Face::SyntaxType,
        Face::SyntaxFunction,
        Face::SyntaxConstant,
        Face::SyntaxProperty,
        Face::SyntaxMacro,
        Face::SyntaxHeading,
        Face::SyntaxLink,
        Face::SyntaxMarkup,
    ];

    /// Highlighted text keeps the distances plain text keeps: a question's words, the marks
    /// and an escape's label are never taken for it (ADR 0111). An escape's label has no box,
    /// so a real `^M` in highlighted text must stand further apart from text that reads `^M`.
    #[test]
    fn highlighted_text_is_in_no_colour_a_question_a_mark_or_an_escape_is_drawn_in() {
        for theme in &themes() {
            for syntax in SYNTAX {
                let ink = theme.foreground(syntax);
                for (face, floor) in [
                    (Face::Question, 0.05),
                    (Face::Whitespace, 0.05),
                    (Face::WrapMark, 0.05),
                    (Face::Escape, 0.10),
                ] {
                    let apart = distance(ink, theme.foreground(face));
                    assert!(
                        apart >= floor,
                        "{}: {syntax:?} and {face:?} {apart:.3}",
                        theme.name()
                    );
                }
            }
        }
    }

    /// The syntax colours as the author accepted them (ADR 0111), in the order of [`SYNTAX`].
    #[test]
    fn the_syntax_faces_keep_the_authors_pick() {
        let picked = [
            (
                "dark",
                [
                    0xb2b0aa, 0xa1a7eb, 0xc695e0, 0x3cbd93, 0xe48eb4, 0x00bbe2, 0xeceef4, 0xfd84bf,
                    0xe69292, 0x0bbcb9, 0xbbaa0c,
                ],
            ),
            (
                "light",
                [
                    0x444345, 0x3837c0, 0x5536a9, 0x384d6b, 0x882757, 0x002ee0, 0x15161b, 0x900636,
                    0x76273f, 0x06525e, 0x504f2e,
                ],
            ),
            (
                "dusk",
                [
                    0xf3f3f6, 0xf3f3f6, 0xf3f3f6, 0x8efede, 0xf3f3f6, 0xb0fdf4, 0xf3f3f6, 0xf3f3f6,
                    0xf3f3f6, 0x76fffb, 0xe2fb68,
                ],
            ),
        ];
        for (theme, (name, colours)) in themes().iter().zip(picked) {
            assert_eq!(theme.name(), name);
            for (face, colour) in SYNTAX.iter().zip(colours) {
                assert_eq!(
                    theme.foreground(*face),
                    rgb(colour).linear(),
                    "{name}: {face:?}"
                );
            }
        }
    }

    #[test]
    fn every_highlight_has_its_own_face() {
        use oso_core::syntax::Highlight;
        let all = [
            Highlight::Comment,
            Highlight::String,
            Highlight::Keyword,
            Highlight::Type,
            Highlight::Function,
            Highlight::Constant,
            Highlight::Property,
            Highlight::Macro,
            Highlight::Heading,
            Highlight::Link,
            Highlight::Markup,
        ];
        assert_eq!(all.map(Face::of_highlight), SYNTAX);
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
                (Face::JumpLabel, Face::JumpLabelNext),
            ] {
                let apart = distance(fill(a), fill(b));
                assert!(apart >= 0.09, "{name}: {a:?} and {b:?} {apart:.3}");
            }
            // A label is never taken for an escape, a box, a match or the region (ADR 0116). An
            // escape has no box, so its label's own colour is what a label's fill must not be.
            for label in [Face::JumpLabel, Face::JumpLabelNext] {
                let colours = [
                    ("Escape's label", theme.foreground(Face::Escape)),
                    ("Boxed", fill(Face::Boxed)),
                    ("SearchMatch", fill(Face::SearchMatch)),
                    ("SearchCurrent", fill(Face::SearchCurrent)),
                    ("Region", fill(Face::Region)),
                ];
                for (what, colour) in colours {
                    let apart = distance(fill(label), colour);
                    assert!(apart >= 0.09, "{name}: {label:?} and {what} {apart:.3}");
                }
            }
        }
    }

    /// A review's person's and agent's labels are told apart by their colour, besides their
    /// words, and the added and removed bands by theirs (ADR 0122).
    #[test]
    fn a_reviews_authors_and_sides_are_told_apart() {
        for theme in &themes() {
            let name = theme.name();
            let labels = distance(
                theme.foreground(Face::ReviewCommentPerson),
                theme.foreground(Face::ReviewCommentAgent),
            );
            assert!(labels >= 0.09, "{name}: the labels {labels:.3}");
            let bands = distance(
                theme.background(Face::DiffAdded),
                theme.background(Face::DiffRemoved),
            );
            assert!(bands >= 0.05, "{name}: the bands {bands:.3}");
        }
    }

    /// A jump's label fills are no other face's, nor near one: whatever else a frame fills or
    /// draws a line in, a label stands apart from it (ADR 0116).
    #[test]
    fn a_labels_fill_is_no_other_faces() {
        for theme in &themes() {
            for label in [Face::JumpLabel, Face::JumpLabelNext] {
                let fill = theme.background(label);
                for face in FACES {
                    if matches!(face, Face::JumpLabel | Face::JumpLabelNext) {
                        continue;
                    }
                    let other = theme.background(face);
                    assert_ne!(fill, other, "{}: {label:?} and {face:?}", theme.name());
                    let apart = distance(fill, other);
                    assert!(
                        apart >= 0.05,
                        "{}: {label:?} and {face:?} {apart:.3}",
                        theme.name()
                    );
                }
            }
        }
    }

    /// A terminal's colours 1 to 6 and 9 to 14 are read on the text's background at 7:1, the
    /// floor highlighted text has there (ADR 0120).
    #[test]
    fn a_terminals_colours_are_read_on_the_text_background() {
        for theme in &themes() {
            let behind = theme.background(Face::Text);
            for n in [1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14] {
                let face = TERMINAL[n];
                let ratio = contrast(over(theme.foreground(face), behind), behind);
                assert!(ratio >= 7.0, "{}: {face:?} {ratio:.2}", theme.name());
            }
        }
    }

    /// The colours a terminal's engine is given are the faces' (ADR 0120).
    #[test]
    fn a_terminal_palette_is_the_faces_colours() {
        let linear = |[r, g, b]: [u8; 3]| rgb(u32::from_be_bytes([0, r, g, b])).linear();
        for theme in &themes() {
            let palette = theme.terminal_palette();
            let name = theme.name();
            assert_eq!(
                linear(palette.foreground),
                theme.foreground(Face::Text),
                "{name}"
            );
            assert_eq!(
                linear(palette.background),
                theme.background(Face::Text),
                "{name}"
            );
            assert_eq!(
                linear(palette.cursor),
                theme.background(Face::Cursor),
                "{name}"
            );
            for (n, colour) in palette.ansi.iter().enumerate() {
                let face = TERMINAL[n];
                assert_eq!(linear(*colour), theme.foreground(face), "{name}: {face:?}");
            }
        }
        let dark = themes()[0].terminal_palette();
        assert_eq!(dark.background, [0x0d, 0x0e, 0x12]);
        assert_eq!(dark.ansi[1], [0xff, 0x8f, 0x87]);
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
