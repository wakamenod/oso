//! libghostty-vt as oso-term's `Engine` (ADR 0120, step 1's choice): snapshots from the
//! render state's dirty rows, packed cells decoded with the bit positions `manifest_tests`
//! checks against the library's manifest, and a style read once per style id per row. It
//! warns once per terminal when oso's patch to the library copied rows without their styling.
//!
//! Must not depend on: the policy or the thread. This file is safe code over `raw`.

mod ffi;
mod raw;

#[cfg(test)]
mod manifest_tests;
#[cfg(test)]
pub(crate) mod tests;

use oso_platform::process::pty::Size;

use super::{Effect, Engine};
use crate::limits;
use crate::snapshot::{
    Attrs, Cell, Color, Cursor, CursorShape, Modes, Mouse, Palette, Rgb, Row, Snapshot, Underline,
};
use raw::Vt;

/// Bit positions in a packed `GhosttyCell` (types.h's manifest), as (lowest bit, width).
mod layout {
    pub const CONTENT_TAG: (u32, u32) = (0, 2);
    pub const CODEPOINT: (u32, u32) = (2, 21);
    pub const PALETTE_INDEX: (u32, u32) = (2, 8);
    pub const RED: (u32, u32) = (2, 8);
    pub const GREEN: (u32, u32) = (10, 8);
    pub const BLUE: (u32, u32) = (18, 8);
    pub const STYLE_ID: (u32, u32) = (26, 16);
    pub const WIDE: (u32, u32) = (42, 2);

    pub fn get(cell: u64, (lsb, width): (u32, u32)) -> u64 {
        (cell >> lsb) & ((1 << width) - 1)
    }
}

/// How many styles a row's cache holds.
const STYLES_CACHED: usize = 16;

/// The DEC private modes the snapshot reports, in `Modes`' order.
const MODES: [u16; 7] = [1, 2004, 1004, 1000, 1002, 1003, 1006];

pub(crate) struct Ghostty {
    vt: Vt,
    size: Size,
    /// Whether the next snapshot must send every row.
    full: bool,
    cursor: Cursor,
    modes: Modes,
    history: usize,
    raw: Vec<ffi::Cell>,
    grapheme: [u8; limits::GRAPHEME_MAX],
    /// The row's styles read so far, as cells, by style id: a direct-mapped cache, cleared
    /// for each row, as style ids belong to a page.
    styles: [(u64, Cell); STYLES_CACHED],
    /// Whether this terminal has warned that rows were copied without their styling.
    warned_plain_copies: bool,
}

impl Ghostty {
    /// A terminal of `size` keeping `scrollback` lines, or `None` if the library cannot
    /// allocate one. `size` is clamped to the limits already.
    /// `xtversion` is what `XTVERSION` answers (the policy's).
    pub(crate) fn new(size: Size, scrollback: usize, xtversion: &'static str) -> Option<Ghostty> {
        let mut vt = Vt::new(size.columns, size.rows, scrollback, xtversion)?;
        let (width, height) = cell_pixels(size);
        vt.resize(size.columns, size.rows, width, height);
        Some(Ghostty {
            vt,
            size,
            full: true,
            cursor: Cursor::default(),
            modes: Modes::default(),
            history: 0,
            raw: Vec::new(),
            grapheme: [0; limits::GRAPHEME_MAX],
            styles: [(0, Cell::default()); STYLES_CACHED],
            warned_plain_copies: false,
        })
    }

    /// Warns once per terminal when the library copied rows without their styles, links or
    /// clusters because they did not fit their page (oso's patch, `patches/`). The terminal
    /// keeps running; the warning holds a count, never text.
    fn note_plain_copies(&mut self) {
        if self.warned_plain_copies {
            return;
        }
        let copies = self.vt.plain_copies().unwrap_or(0);
        if copies > 0 {
            self.warned_plain_copies = true;
            log::warn!(
                "a terminal copied {copies} rows without their styling: their page was full"
            );
        }
    }

    /// The settings bounding memory, for the tests: the scrollback's bytes, then the Kitty
    /// image store's limit and its file and shared-memory mediums.
    #[cfg(test)]
    pub(crate) fn memory_settings(&self) -> (usize, u64, bool, bool) {
        raw::memory_settings(&self.vt)
    }

    fn build_row(&mut self, index: u16, row: &mut Row) {
        row.index = index;
        row.wrapped = self.vt.row(&mut self.raw);
        row.cells.clear();
        row.text.clear();
        row.graphemes.clear();
        // Id 0 never enters the cache: it is the default style.
        self.styles.fill((0, Cell::default()));
        let columns = usize::from(self.size.columns);
        for (column, &packed) in self.raw.iter().take(columns).enumerate() {
            let column = u16::try_from(column).unwrap_or(u16::MAX);
            let id = layout::get(packed, layout::STYLE_ID);
            let mut cell = if id == 0 {
                Cell::default()
            } else {
                let slot = &mut self.styles[id as usize % STYLES_CACHED];
                if slot.0 != id {
                    *slot = (id, from_style(&self.vt.style(column)));
                }
                slot.1
            };
            let tag = layout::get(packed, layout::CONTENT_TAG) as i32;
            match tag {
                ffi::CELL_CONTENT_CODEPOINT | ffi::CELL_CONTENT_CODEPOINT_GRAPHEME => {
                    let code = layout::get(packed, layout::CODEPOINT) as u32;
                    if code != 0 {
                        cell.ch = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
                    }
                }
                ffi::CELL_CONTENT_BG_COLOR_PALETTE => {
                    cell.bg = Color::Indexed(layout::get(packed, layout::PALETTE_INDEX) as u8);
                }
                _ => {
                    cell.bg = Color::Rgb(Rgb {
                        r: layout::get(packed, layout::RED) as u8,
                        g: layout::get(packed, layout::GREEN) as u8,
                        b: layout::get(packed, layout::BLUE) as u8,
                    });
                }
            }
            match layout::get(packed, layout::WIDE) as i32 {
                ffi::CELL_WIDE_WIDE => cell.attrs = cell.attrs.with(Attrs::WIDE),
                ffi::CELL_WIDE_SPACER_TAIL => cell.attrs = cell.attrs.with(Attrs::WIDE_SPACER),
                _ => {}
            }
            // A cluster the library cannot give stays its first code point.
            let cluster = (tag == ffi::CELL_CONTENT_CODEPOINT_GRAPHEME)
                .then(|| self.vt.grapheme(column, &mut self.grapheme))
                .flatten();
            if let Some(len) = cluster {
                cell.attrs = cell.attrs.with(Attrs::GRAPHEME);
                // Borrowed, not copied, when the bytes are UTF-8, as the library's are.
                let start = row.text.len();
                row.text
                    .push_str(&String::from_utf8_lossy(&self.grapheme[..len]));
                // At most columns × GRAPHEME_MAX × 3 bytes (each byte lossy-replaced): u32.
                let range = start as u32..row.text.len() as u32;
                row.graphemes.push((column, range));
            }
            row.cells.push(cell);
        }
        row.cells.resize(columns, Cell::default());
    }

    fn read_cursor(frame: &raw::Frame) -> Cursor {
        let c = &frame.cursor;
        Cursor {
            row: c.viewport_y,
            column: c.viewport_x,
            visible: c.visible && c.viewport_has_value,
            blinking: c.blinking,
            shape: match c.visual_style {
                ffi::CURSOR_STYLE_BAR => CursorShape::Bar,
                ffi::CURSOR_STYLE_UNDERLINE => CursorShape::Underline,
                ffi::CURSOR_STYLE_BLOCK_HOLLOW => CursorShape::HollowBlock,
                _ => CursorShape::Block,
            },
        }
    }

    fn read_modes(&self) -> (Modes, usize) {
        let state = self.vt.state();
        let [cursor_keys, paste, focus, click, drag, motion, sgr] = self.vt.dec_modes(MODES);
        let mouse = if motion {
            Mouse::Motion
        } else if drag {
            Mouse::Drag
        } else if click {
            Mouse::Click
        } else {
            Mouse::Off
        };
        let modes = Modes {
            alternate_screen: state.alternate_screen,
            application_cursor: cursor_keys,
            bracketed_paste: paste,
            focus_reports: focus,
            mouse,
            sgr_mouse: sgr,
            kitty_keyboard: state.kitty_keyboard,
        };
        (modes, state.history)
    }
}

impl Engine for Ghostty {
    fn feed(&mut self, bytes: &[u8]) {
        self.vt.write(bytes);
        self.note_plain_copies();
    }

    fn resize(&mut self, size: Size) {
        let (width, height) = cell_pixels(size);
        self.vt.resize(size.columns, size.rows, width, height);
        self.size = size;
        self.full = true;
        self.note_plain_copies();
    }

    fn set_palette(&mut self, palette: &Palette) {
        let rgb = |c: Rgb| ffi::ColorRgb {
            r: c.r,
            g: c.g,
            b: c.b,
        };
        let colors = xterm_palette(&palette.ansi).map(rgb);
        let set = self.vt.set_colors(
            rgb(palette.foreground),
            rgb(palette.background),
            palette.cursor.map(rgb),
            &colors,
        );
        debug_assert!(set);
        self.full = true;
    }

    fn snapshot(&mut self, out: &mut Snapshot, spare: &mut Vec<Row>) -> bool {
        self.vt.update();
        let frame = self.vt.frame();
        let full = std::mem::take(&mut self.full) || frame.dirty == ffi::RENDER_DIRTY_FULL;
        let mut changed = false;
        if full || frame.dirty == ffi::RENDER_DIRTY_PARTIAL {
            let mut index = 0;
            while index < frame.rows && self.vt.next_row() {
                if full || self.vt.row_dirty() {
                    let mut row = spare.pop().unwrap_or_default();
                    self.build_row(index, &mut row);
                    out.rows.push(row);
                    changed = true;
                }
                index += 1;
            }
        }
        self.vt.clean();
        let cursor = Self::read_cursor(&frame);
        let (modes, history) = self.read_modes();
        changed |= cursor != self.cursor || modes != self.modes || history != self.history;
        (self.cursor, self.modes, self.history) = (cursor, modes, history);
        out.size = self.size;
        out.cursor = cursor;
        out.modes = modes;
        out.history = history;
        changed
    }

    fn take_effects(&mut self, out: &mut Vec<Effect>) {
        self.vt.sink().take(out);
    }
}

/// A cell's colors and attributes from its style.
fn from_style(style: &ffi::Style) -> Cell {
    const FLAGS: [(u16, Attrs); 8] = [
        (ffi::STYLE_FLAG_BOLD, Attrs::BOLD),
        (ffi::STYLE_FLAG_FAINT, Attrs::DIM),
        (ffi::STYLE_FLAG_ITALIC, Attrs::ITALIC),
        (ffi::STYLE_FLAG_BLINK, Attrs::BLINK),
        (ffi::STYLE_FLAG_INVERSE, Attrs::INVERSE),
        (ffi::STYLE_FLAG_INVISIBLE, Attrs::HIDDEN),
        (ffi::STYLE_FLAG_STRIKETHROUGH, Attrs::STRIKE),
        (ffi::STYLE_FLAG_OVERLINE, Attrs::OVERLINE),
    ];
    let attrs = FLAGS
        .iter()
        .filter(|(bit, _)| style.flags & bit != 0)
        .fold(Attrs::default(), |all, &(_, attr)| all.with(attr));
    let underline =
        match i32::from((style.flags & ffi::STYLE_UNDERLINE_MASK) >> ffi::STYLE_UNDERLINE_SHIFT) {
            ffi::UNDERLINE_NONE => Underline::None,
            ffi::UNDERLINE_DOUBLE => Underline::Double,
            ffi::UNDERLINE_CURLY => Underline::Curly,
            ffi::UNDERLINE_DOTTED => Underline::Dotted,
            ffi::UNDERLINE_DASHED => Underline::Dashed,
            _ => Underline::Single,
        };
    Cell {
        ch: ' ',
        fg: color(&style.fg_color),
        bg: color(&style.bg_color),
        underline_color: color(&style.underline_color),
        underline,
        attrs,
    }
}

fn color(c: &ffi::StyleColor) -> Color {
    let [a, b, c2] = c.value;
    match c.tag {
        ffi::STYLE_COLOR_PALETTE => Color::Indexed(a),
        ffi::STYLE_COLOR_RGB => Color::Rgb(Rgb { r: a, g: b, b: c2 }),
        _ => Color::Default,
    }
}

/// A cell's size in pixels, for size reports; 0 when the size has no pixels.
fn cell_pixels(size: Size) -> (u32, u32) {
    let per = |pixels: u16, cells: u16| u32::from(pixels) / u32::from(cells.max(1));
    (per(size.width, size.columns), per(size.height, size.rows))
}

/// xterm's 256 colors, with the theme's 16 first: a 6 × 6 × 6 cube, then 24 grays.
fn xterm_palette(ansi: &[Rgb; 16]) -> [Rgb; 256] {
    let mut colors = [Rgb::default(); 256];
    colors[..16].copy_from_slice(ansi);
    let level = |n: usize| if n == 0 { 0 } else { (55 + n * 40) as u8 };
    for (i, color) in colors[16..232].iter_mut().enumerate() {
        *color = Rgb {
            r: level(i / 36),
            g: level(i / 6 % 6),
            b: level(i % 6),
        };
    }
    for (i, color) in colors[232..].iter_mut().enumerate() {
        let gray = (8 + i * 10) as u8;
        *color = Rgb {
            r: gray,
            g: gray,
            b: gray,
        };
    }
    colors
}
