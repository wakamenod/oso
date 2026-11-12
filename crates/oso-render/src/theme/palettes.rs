//! The built-in themes' palettes (ADR 0027, ADR 0089): every name a face map refers to, and
//! the colour each theme gives it.
//!
//! Must not depend on: `atlas`, `grid`, `rows`, `scene`, `shape`, `text` or the GPU.

use super::{Color, rgb, rgba};

/// The names of a palette's colours. A face map refers to these, and each theme's palette
/// gives every one a colour (ADR 0027's two layers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Name {
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
    /// The cursor flash, at full strength.
    BgFlash,
    /// Line numbers, dim, and the current line's, bright.
    FgLineNumber,
    FgLineNumberCurrent,
    /// The wrap mark: dim, as whitespace's marks, in a hue of its own.
    FgWrapMark,
    /// The selected window's modeline text.
    FgMode,
    /// A modeline of a window that is not selected: dimmer than the selected one's.
    FgModeInactive,
    BgModeInactive,
    /// The line between side by side windows.
    Divider,
    /// What highlighting says text is (ADR 0111): oso's own colours, computed in OKLab for each
    /// class with the look of a vivid syntax mapping the author picked, each just light (or dark)
    /// enough for 7:1 on the background and the current line's band. Provisional, until the
    /// author's own syntax theme replaces them. Fields are drawn as text, in `FgMain`.
    SynComment,
    SynString,
    SynKeyword,
    SynType,
    SynFunction,
    SynConstant,
    SynMacro,
    SynHeading,
    SynLink,
    SynMarkup,
}

/// Near-black, with a high-contrast foreground: the default.
pub(super) fn dark(name: Name) -> Color {
    use Name::*;
    match name {
        BgMain => rgb(0x0d0e12),
        BgFloat => rgb(0x181a22),
        BgKey => rgb(0x2b2e3b),
        // The selected window's modeline in blue, far apart from the others' (ADR 0112).
        BgMode => rgb(0x2c3f6e),
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
        BgFlash => rgb(0x4a2a5a),
        FgLineNumber => rgb(0x767c93),
        FgLineNumberCurrent => rgb(0xd8dbe6),
        FgWrapMark => rgb(0x6aa6c4),
        FgMode => rgb(0xffffff),
        FgModeInactive => rgb(0x8e95ad),
        BgModeInactive => rgb(0x15171e),
        Divider => rgb(0x60667e),
        Hairline => rgba(0xffffff, 0.05),
        Border => rgb(0x30344a),
        Shadow => rgba(0x000000, 0.6),
        SynComment => rgb(0xb2b0aa),
        SynString => rgb(0xa1a7eb),
        SynKeyword => rgb(0xc695e0),
        SynType => rgb(0x3cbd93),
        SynFunction => rgb(0xe48eb4),
        SynConstant => rgb(0x00bbe2),
        SynMacro => rgb(0xfd84bf),
        SynHeading => rgb(0xe69292),
        SynLink => rgb(0x0bbcb9),
        SynMarkup => rgb(0xbbaa0c),
    }
}

/// Near-white, with a near-black foreground: `dark`'s pair.
pub(super) fn light(name: Name) -> Color {
    use Name::*;
    match name {
        BgMain => rgb(0xfbfbf8),
        BgFloat => rgb(0xeff0f5),
        BgKey => rgb(0xdadeea),
        BgMode => rgb(0xb8cdf2),
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
        BgFlash => rgb(0xebc9ef),
        FgLineNumber => rgb(0x6e7385),
        FgLineNumberCurrent => rgb(0x272a33),
        FgWrapMark => rgb(0x2f6684),
        FgMode => rgb(0x0b0c10),
        FgModeInactive => rgb(0x5a6178),
        BgModeInactive => rgb(0xeef0f4),
        Divider => rgb(0x858a9b),
        Hairline => rgba(0x000000, 0.08),
        Border => rgb(0xc2c6d4),
        Shadow => rgba(0x1a1f33, 0.18),
        SynComment => rgb(0x444345),
        SynString => rgb(0x3837c0),
        SynKeyword => rgb(0x5536a9),
        SynType => rgb(0x384d6b),
        SynFunction => rgb(0x882757),
        SynConstant => rgb(0x002ee0),
        SynMacro => rgb(0x900636),
        SynHeading => rgb(0x76273f),
        SynLink => rgb(0x06525e),
        SynMarkup => rgb(0x504f2e),
    }
}

/// The M2 steps' dark grey, with the float's colours as ADR 0075 and ADR 0076 picked them.
/// Where those missed a floor, the nearest colour that meets it, its hue kept (ADR 0089).
pub(super) fn dusk(name: Name) -> Color {
    use Name::*;
    match name {
        BgMain => rgb(0x3f3f4b),
        BgFloat => rgb(0x474959),
        BgKey => rgb(0x555769),
        BgMode => rgb(0x44507a),
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
        BgFlash => rgb(0x603c6c),
        FgLineNumber => rgb(0xb0b3c4),
        FgLineNumberCurrent => rgb(0xf3f3f6),
        FgWrapMark => rgb(0x94cde6),
        FgMode => rgb(0xf3f3f6),
        FgModeInactive => rgb(0xb8bccc),
        BgModeInactive => rgb(0x35353f),
        Divider => rgb(0x8c8fa3),
        Hairline => rgba(0xffffff, 0.035),
        Border => rgb(0x505265),
        Shadow => rgba(0x08090e, 0.55),
        SynComment => rgb(0xf3f3f6),
        SynString => rgb(0xf3f3f6),
        SynKeyword => rgb(0xf3f3f6),
        SynType => rgb(0x8efede),
        SynFunction => rgb(0xf3f3f6),
        SynConstant => rgb(0xb0fdf4),
        SynMacro => rgb(0xf3f3f6),
        SynHeading => rgb(0xf3f3f6),
        SynLink => rgb(0x76fffb),
        SynMarkup => rgb(0xe2fb68),
    }
}
