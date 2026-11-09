//! Palettes for the themes still to be picked (ADR 0089): `dark` and `light`, and variants of
//! their contested colours, drawn as sample frames for the author. Tests only; a picked one
//! moves into the theme list.
//!
//! Must not depend on: `atlas`, `grid`, `rows`, `scene`, `shape`, `text` or the GPU.

use super::{Color, Kind, Name, Source, TextGamma, Theme, dusk, faces, rgb, rgba};

/// `dusk` with the nearest colours that meet the floors where its own miss them: each one only
/// darker or lighter in OKLab, its hue kept.
fn dusk_nearest(name: Name) -> Color {
    use Name::*;
    match name {
        BgRegion => rgb(0x3f5375),
        BgCurrent => rgb(0x7b4500),
        BgEscape => rgb(0x6b452b),
        BgMode => rgb(0x51515f),
        BgSelected => rgb(0x293d78),
        FgQuestion => rgb(0xdce4fd),
        other => dusk(other),
    }
}

/// Near-black, with a high-contrast foreground.
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
        BgCurrent => rgb(0x684a00),
        BgSelected => rgb(0x1e3366),
        FgMain => rgb(0xeceef4),
        FgPrompt => rgb(0xa6b0d0),
        FgKey => rgb(0xe2e6f2),
        FgQuestion => rgb(0xb8dcff),
        Escape => rgb(0xf2a866),
        Warning => rgb(0xf5c84c),
        FieldOutline => rgb(0x5a76c2),
        Gold => rgb(0xf7c96e),
        Slate => rgb(0x9fabd2),
        Azure => rgb(0x6cc4ff),
        Teal => rgb(0x5ad6ae),
        Hairline => rgba(0xffffff, 0.05),
        Border => rgb(0x30344a),
        Shadow => rgba(0x000000, 0.6),
    }
}

/// `dark` with the current isearch match in another hue than the others' amber.
fn dark_teal_current(name: Name) -> Color {
    match name {
        Name::BgCurrent => rgb(0x00566a),
        other => dark(other),
    }
}

/// Near-white, with a near-black foreground.
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
        BgCurrent => rgb(0xf0c040),
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
        Hairline => rgba(0x000000, 0.08),
        Border => rgb(0xc2c6d4),
        Shadow => rgba(0x1a1f33, 0.18),
    }
}

/// `light` with the current isearch match in another hue than the others' amber.
fn light_teal_current(name: Name) -> Color {
    match name {
        Name::BgCurrent => rgb(0x8fdcd0),
        other => light(other),
    }
}

const SOURCES: [Source; 5] = [
    candidate("dusk-nearest", Kind::Dark, dusk_nearest),
    candidate("dark", Kind::Dark, dark),
    candidate("dark-teal-current", Kind::Dark, dark_teal_current),
    candidate("light", Kind::Light, light),
    candidate("light-teal-current", Kind::Light, light_teal_current),
];

const fn candidate(name: &'static str, kind: Kind, palette: fn(Name) -> Color) -> Source {
    Source {
        name,
        kind,
        palette,
        faces,
        text: TextGamma { strength: 0.0 },
    }
}

/// Every candidate, resolved as a built-in theme would be.
pub(crate) fn candidates() -> Vec<Theme> {
    SOURCES.iter().map(Theme::resolve).collect()
}
