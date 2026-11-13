//! A terminal's colours and attributes as oso-render draws them (ADR 0120): the default and the
//! 16 named colours through the theme's faces, so a theme switch needs no new rows, the 256
//! colours past them as xterm gives them, and true colour as the program set it.
//!
//! Must not depend on: `app`; this only converts.

use oso_render::theme::TerminalPalette;
use oso_render::{Face, GridColor, GridStyle};
use oso_term::{Attrs, Cell, Color, Palette, Rgb, Underline};

/// The faces of the 16 named colours, by number.
const FACES: [Face; 16] = [
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

/// Colours for a renderer with no theme, which never happens: grey on black.
pub(crate) const FALLBACK: TerminalPalette = TerminalPalette {
    foreground: [0xc0; 3],
    background: [0; 3],
    cursor: [0xc0; 3],
    ansi: [[0x80; 3]; 16],
};

/// `color` as oso-render draws it.
pub(crate) fn color(color: Color) -> GridColor {
    match color {
        Color::Default => GridColor::Default,
        Color::Indexed(n) => match FACES.get(usize::from(n)) {
            Some(face) => GridColor::Face(*face),
            None => GridColor::Rgb(indexed(n)),
        },
        Color::Rgb(Rgb { r, g, b }) => GridColor::Rgb([r, g, b]),
    }
}

/// xterm's colour `n` from 16 on: the 6 × 6 × 6 cube, then 24 greys.
fn indexed(n: u8) -> [u8; 3] {
    if n >= 232 {
        let grey = 8 + 10 * (n - 232);
        return [grey; 3];
    }
    let i = n.saturating_sub(16);
    let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
    [level(i / 36), level((i / 6) % 6), level(i % 6)]
}

/// A cell's attributes and underline as oso-render's style. Blink and overline are not drawn.
pub(crate) fn style(cell: &Cell) -> GridStyle {
    let pairs = [
        (Attrs::BOLD, GridStyle::BOLD),
        (Attrs::ITALIC, GridStyle::ITALIC),
        (Attrs::DIM, GridStyle::DIM),
        (Attrs::INVERSE, GridStyle::INVERSE),
        (Attrs::HIDDEN, GridStyle::HIDDEN),
        (Attrs::STRIKE, GridStyle::STRIKE),
        (Attrs::WIDE, GridStyle::WIDE),
        (Attrs::WIDE_SPACER, GridStyle::WIDE_SPACER),
        (Attrs::GRAPHEME, GridStyle::GRAPHEME),
    ];
    let mut style = GridStyle::default();
    for (attr, drawn) in pairs {
        if cell.attrs.contains(attr) {
            style = style.with(drawn);
        }
    }
    match cell.underline {
        Underline::None => style,
        Underline::Double => style.with(GridStyle::DOUBLE_UNDERLINE),
        // Curly, dotted and dashed underlines draw as one line for now.
        Underline::Single | Underline::Curly | Underline::Dotted | Underline::Dashed => {
            style.with(GridStyle::UNDERLINE)
        }
    }
}

/// The theme's colours, for the engine to answer colour queries from (OSC 4, 10, 11, 12).
pub(crate) fn palette(theme: &TerminalPalette) -> Palette {
    let rgb = |[r, g, b]: [u8; 3]| Rgb { r, g, b };
    Palette {
        foreground: rgb(theme.foreground),
        background: rgb(theme.background),
        cursor: Some(rgb(theme.cursor)),
        ansi: theme.ansi.map(rgb),
    }
}

#[cfg(test)]
mod tests {
    use oso_render::{Face, GridColor, GridStyle};
    use oso_term::{Attrs, Cell, Color, Rgb, Underline};

    use super::{color, indexed, style};

    #[test]
    fn named_colours_are_the_theme_s_faces_and_the_rest_are_drawn_as_given() {
        assert_eq!(color(Color::Default), GridColor::Default);
        assert_eq!(
            color(Color::Indexed(1)),
            GridColor::Face(Face::TerminalColor1)
        );
        assert_eq!(
            color(Color::Indexed(15)),
            GridColor::Face(Face::TerminalColor15)
        );
        assert_eq!(
            color(Color::Rgb(Rgb { r: 1, g: 2, b: 3 })),
            GridColor::Rgb([1, 2, 3])
        );
    }

    #[test]
    fn the_256_colours_are_xterm_s() {
        assert_eq!(indexed(16), [0, 0, 0]);
        assert_eq!(indexed(21), [0, 0, 255]);
        assert_eq!(indexed(196), [255, 0, 0]);
        assert_eq!(indexed(231), [255, 255, 255]);
        assert_eq!(indexed(232), [8, 8, 8]);
        assert_eq!(indexed(255), [238, 238, 238]);
    }

    #[test]
    fn attributes_and_underlines_carry_over() {
        let cell = Cell {
            attrs: Attrs::BOLD,
            underline: Underline::Curly,
            ..Cell::default()
        };
        let drawn = style(&cell);
        assert!(drawn.contains(GridStyle::BOLD));
        assert!(!drawn.contains(GridStyle::INVERSE));
        let inverse = Cell {
            attrs: Attrs::INVERSE,
            ..Cell::default()
        };
        assert!(style(&inverse).contains(GridStyle::INVERSE));
        assert!(drawn.contains(GridStyle::UNDERLINE));
        assert!(!drawn.contains(GridStyle::DOUBLE_UNDERLINE));
        let double = Cell {
            underline: Underline::Double,
            ..Cell::default()
        };
        assert!(style(&double).contains(GridStyle::DOUBLE_UNDERLINE));
    }
}
