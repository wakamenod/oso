//! A terminal's grid rows read for the shaping cache, and the colours each cell is drawn in
//! (ADR 0120).
//!
//! Must not depend on: `scene`, `text`, `paint`, `capture` or the GPU. It fills what the
//! shaping cache shapes, and the scene draws what comes of it.
//!
//! - Each cell keeps its own column, as the terminal placed it: its text is a display cluster
//!   of its cells, so widths are never measured again (ADR 0035).
//! - A piece to shape is a run of cells of one style and colour, so a row of output is a few
//!   calls to the shaper. Its blank cells are spaces in it, but those at its end, which are
//!   never shaped, and they draw no glyph. A cell holding a right-to-left character is a piece
//!   of its own, so the shaper cannot reorder it with its neighbours (ADR 0066).
//! - A control or invisible character never reaches the shaper (security.md, T3). Its cell is
//!   boxed, as an invisible character is in a text view, and so is a cell whose text a text
//!   view would not show as one cluster, such as a letter an invisible character extends: it is
//!   drawn as its first character in the box, if that one shows, as a cut cluster is
//!   (ADR 0068). The box keeps the terminal's cells, so it cannot hold a code point's label.
//! - A row has a budget of characters per column, as a text view's has. Past it, a cell of
//!   several characters is cut to its first, in a box (ADR 0068).

use oso_core::display_map::cell::is_invisible;
use oso_core::display_map::{CellKind, Clusters, MAX_CLUSTER_BYTES};

use crate::grid::Grapheme;
use crate::pane::{Face, GridCell, GridColor, GridRow, GridStyle};
use crate::rows::{CHARS_PER_COLUMN, RowInput};
use crate::theme::{Theme, decode};

/// What a cell's text is shaped as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shaped<'a> {
    /// In the piece of the cells before it of its style and colour.
    Run,
    /// In a piece of its own.
    Alone,
    /// In a box, as its first character if that one shows.
    Boxed(Option<&'a str>),
}

/// Whether `c` is in a block of a right-to-left script, as the text system splits runs by
/// direction (`oso_platform::text`). Such a cell is shaped alone.
fn is_right_to_left(c: char) -> bool {
    matches!(
        u32::from(c),
        0x590..=0x8FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF
    )
}

/// Whether `c` is drawn as an escape or a boxed code point in a text view.
fn is_hidden(c: char) -> bool {
    c.is_control() || is_invisible(c)
}

/// How a cell's `text`, which is not empty, is shaped. `whole` says the row has budget left
/// for a cell of several characters.
fn shaped_as(text: &str, whole: bool) -> Shaped<'_> {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return Shaped::Run;
    };
    if chars.next().is_none() {
        return if is_hidden(first) {
            Shaped::Boxed(None)
        } else if is_right_to_left(first) {
            Shaped::Alone
        } else {
            Shaped::Run
        };
    }
    let cut = Shaped::Boxed((!is_hidden(first)).then(|| &text[..first.len_utf8()]));
    if !whole || text.len() > MAX_CLUSTER_BYTES {
        return cut;
    }
    // One display cluster of text, as a text view shows it, or the cut.
    match Clusters::new(text).next() {
        Some((_, cluster, CellKind::Text, _)) if cluster.len() == text.len() => {
            if text.chars().any(is_right_to_left) {
                Shaped::Alone
            } else {
                Shaped::Run
            }
        }
        _ => cut,
    }
}

/// The text of the cell at `column` from `row`'s graphemes, searched from `next`, which this
/// moves past it. `None` if the row gives none, or bytes that are not its text's.
fn grapheme_at<'a>(row: &GridRow<'a>, column: usize, next: &mut usize) -> Option<&'a str> {
    while let Some((at, bytes)) = row.graphemes.get(*next) {
        let at = usize::from(*at);
        if at > column {
            return None;
        }
        *next += 1;
        if at == column {
            return row
                .text
                .get(bytes.start as usize..bytes.end as usize)
                .filter(|text| !text.is_empty());
        }
    }
    None
}

/// What a run of cells shares: its colours and the flags that change how it looks, leaving
/// out those that say only how wide a cell is or where its text is.
fn look(cell: &GridCell) -> (GridColor, GridColor, GridColor, GridStyle) {
    let layout = GridStyle::WIDE
        .with(GridStyle::WIDE_SPACER)
        .with(GridStyle::GRAPHEME);
    (
        cell.fg,
        cell.bg,
        cell.underline_color,
        cell.style.without(layout),
    )
}

/// The cells a cell's glyph or box takes: two for a wide one.
pub(crate) fn cells_of(cell: &GridCell) -> usize {
    if cell.style.contains(GridStyle::WIDE) {
        2
    } else {
        1
    }
}

/// Whether `cell` draws no glyph: a space, or one never written.
pub(crate) fn is_blank(cell: &GridCell) -> bool {
    matches!(cell.ch, ' ' | '\0') && !cell.style.contains(GridStyle::GRAPHEME)
}

/// What a row is read into, and where its reading is.
struct Reader<'a> {
    input: RowInput<'a>,
    /// The characters the row may shape before a cell of several is cut, and those it has.
    budget: usize,
    spent: usize,
    /// The look of the piece being read, or `None` if the next cell starts one.
    piece: Option<(GridColor, GridColor, GridColor, GridStyle)>,
    /// The columns of the blank cells since the piece's last glyph, which stay in it, as
    /// spaces, only if a glyph of its look follows them. Trailing ones are never shaped.
    blanks: Option<(usize, usize)>,
}

impl Reader<'_> {
    /// Adds `text` to shape, a cluster of `cells` cells at `column`, in `face`.
    fn push(&mut self, column: usize, cells: usize, text: &str, face: Face) {
        let input = &mut self.input;
        input.graphemes.push(Grapheme {
            byte: input.text.len() as u32,
            column,
            cells: cells as u32,
            face,
        });
        input.text.push_str(text);
        self.spent += text.chars().count();
    }

    /// Ends the piece being read: what comes next starts another.
    fn end_piece(&mut self) {
        self.piece = None;
        self.blanks = None;
    }

    /// Starts a piece at the end of the text.
    fn start_piece(&mut self) {
        let input = &mut self.input;
        input.breaks.push(input.text.len() as u32);
    }

    /// A blank cell at `column`: it stays in the piece if it has the piece's look.
    fn blank(&mut self, column: usize, cell: &GridCell) {
        if self.piece == Some(look(cell)) {
            let start = self.blanks.map_or(column, |(start, _)| start);
            self.blanks = Some((start, column + 1));
        } else {
            self.end_piece();
        }
    }

    /// A cell shaped with those of its look before it, with the blanks between them.
    fn run(&mut self, column: usize, cell: &GridCell, text: &str) {
        if self.piece == Some(look(cell)) {
            if let Some((start, end)) = self.blanks.take() {
                for blank in start..end {
                    self.push(blank, 1, " ", Face::Text);
                }
            }
        } else {
            self.end_piece();
            self.piece = Some(look(cell));
            self.start_piece();
        }
        self.push(column, cells_of(cell), text, Face::Text);
    }

    /// A cell shaped alone, `text` in `face`, if there is text.
    fn alone(&mut self, column: usize, cell: &GridCell, text: Option<&str>, face: Face) {
        self.end_piece();
        if let Some(text) = text {
            self.start_piece();
            self.push(column, cells_of(cell), text, face);
        }
    }
}

/// Reads the first `columns` cells of `row` into `input`.
pub(crate) fn read_row(row: &GridRow<'_>, columns: usize, input: RowInput<'_>) {
    let mut reader = Reader {
        input,
        budget: columns.saturating_mul(CHARS_PER_COLUMN),
        spent: 0,
        piece: None,
        blanks: None,
    };
    let mut next = 0;
    let mut buffer = [0; 4];
    for (column, cell) in row.cells.iter().enumerate().take(columns) {
        if cell.style.contains(GridStyle::WIDE_SPACER) {
            continue;
        }
        if is_blank(cell) {
            reader.blank(column, cell);
            continue;
        }
        let own = (cell.style.contains(GridStyle::GRAPHEME))
            .then(|| grapheme_at(row, column, &mut next))
            .flatten();
        let text = own.unwrap_or_else(|| &*cell.ch.encode_utf8(&mut buffer));
        match shaped_as(text, reader.spent < reader.budget) {
            Shaped::Run => reader.run(column, cell, text),
            Shaped::Alone => reader.alone(column, cell, Some(text), Face::Text),
            Shaped::Boxed(first) => {
                reader.input.boxes.push((column, cells_of(cell)));
                reader.alone(column, cell, first, Face::Boxed);
            }
        }
    }
}

/// sRGB bytes in linear light, worked out once, as cells of true colour are resolved each
/// frame.
pub(crate) struct Decode([f32; 256]);

impl Default for Decode {
    fn default() -> Self {
        Self(std::array::from_fn(|byte| decode(byte as f32 / 255.0)))
    }
}

impl Decode {
    fn rgb(&self, [r, g, b]: [u8; 3]) -> [f32; 4] {
        let channel = |byte: u8| self.0[usize::from(byte)];
        [channel(r), channel(g), channel(b), 1.0]
    }
}

/// The colours a cell is drawn in, linear: its text's, its background's if it fills one, and
/// its underline's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ink {
    pub(crate) fg: [f32; 4],
    pub(crate) bg: Option<[f32; 4]>,
    pub(crate) underline: [f32; 4],
}

impl Ink {
    /// The background the cell's text is on.
    pub(crate) fn paper(&self, theme: &Theme) -> [f32; 4] {
        self.bg.unwrap_or_else(|| theme.background(Face::Text))
    }
}

/// What `cell` is drawn in, in `theme`. `INVERSE` swaps the text's colour and the
/// background's, then `DIM` takes the text's halfway to the background's, in linear light as
/// the GPU blends, and `SELECTED` takes the region's colours instead. A cell whose background
/// is [`GridColor::Default`] and not swapped fills none: the pane's text background is behind
/// it.
pub(crate) fn ink(cell: &GridCell, theme: &Theme, decode: &Decode) -> Ink {
    let color = |color: GridColor, default: [f32; 4]| match color {
        GridColor::Default => default,
        GridColor::Face(face) => theme.foreground(face),
        GridColor::Rgb(rgb) => decode.rgb(rgb),
    };
    let text_bg = theme.background(Face::Text);
    let mut fg = color(cell.fg, theme.foreground(Face::Text));
    let mut bg = (cell.bg != GridColor::Default).then(|| color(cell.bg, text_bg));
    if cell.style.contains(GridStyle::INVERSE) {
        let back = bg.unwrap_or(text_bg);
        bg = Some(fg);
        fg = back;
    }
    if cell.style.contains(GridStyle::DIM) {
        let under = bg.unwrap_or(text_bg);
        fg = [0, 1, 2, 3].map(|i| (fg[i] + under[i]) / 2.0);
    }
    // The selection reads as the region does, so its colours are the theme's checked pair.
    if cell.style.contains(GridStyle::SELECTED) {
        fg = theme.foreground(Face::Region);
        bg = Some(theme.background(Face::Region));
    }
    Ink {
        fg,
        bg,
        underline: color(cell.underline_color, fg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(ch: char) -> GridCell {
        GridCell {
            ch,
            fg: GridColor::Default,
            bg: GridColor::Default,
            underline_color: GridColor::Default,
            style: GridStyle::default(),
        }
    }

    #[test]
    fn a_cell_is_shaped_as_a_text_view_would_show_it() {
        assert_eq!(shaped_as("a", true), Shaped::Run);
        assert_eq!(shaped_as("\u{5d0}", true), Shaped::Alone, "Hebrew");
        assert_eq!(shaped_as("\u{202e}", true), Shaped::Boxed(None));
        assert_eq!(shaped_as("\u{7}", true), Shaped::Boxed(None));
        assert_eq!(shaped_as("e\u{301}", true), Shaped::Run);
        assert_eq!(shaped_as("\u{5d1}\u{5bc}", true), Shaped::Alone);
        assert_eq!(shaped_as("a\u{200b}", true), Shaped::Boxed(Some("a")));
        assert_eq!(shaped_as("\u{200b}a", true), Shaped::Boxed(None));
        assert_eq!(shaped_as("e\u{301}", false), Shaped::Boxed(Some("e")));
        let long = format!("e{}", "\u{301}".repeat(40));
        assert_eq!(shaped_as(&long, true), Shaped::Boxed(Some("e")));
        // An emoji sequence is one cluster, its joiners and selector in it.
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        assert_eq!(shaped_as(family, true), Shaped::Run);
    }

    #[test]
    fn graphemes_out_of_place_or_off_the_text_give_none() {
        let graphemes = [(1, 0..3), (3, 2..9), (5, 3..3)];
        let row = GridRow {
            generation: 0,
            cells: &[],
            text: "e\u{301}x",
            graphemes: &graphemes,
        };
        let mut next = 0;
        assert_eq!(grapheme_at(&row, 0, &mut next), None);
        assert_eq!(grapheme_at(&row, 1, &mut next), Some("e\u{301}"));
        assert_eq!(grapheme_at(&row, 3, &mut next), None, "past the text");
        assert_eq!(grapheme_at(&row, 5, &mut next), None, "empty");
        assert_eq!(grapheme_at(&row, 6, &mut next), None);
    }

    #[test]
    fn colours_resolve_swap_and_dim() {
        let theme = crate::theme::themes()[0];
        let decode = Decode::default();
        let text = (theme.foreground(Face::Text), theme.background(Face::Text));
        let plain = ink(&cell('a'), &theme, &decode);
        assert_eq!(
            (plain.fg, plain.bg, plain.underline),
            (text.0, None, text.0)
        );
        let red = GridCell {
            fg: GridColor::Face(Face::TerminalColor1),
            bg: GridColor::Rgb([255, 255, 255]),
            ..cell('a')
        };
        let drawn = ink(&red, &theme, &decode);
        assert_eq!(drawn.fg, theme.foreground(Face::TerminalColor1));
        assert_eq!(drawn.bg, Some([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(drawn.underline, drawn.fg, "the text's colour");
        let swapped = ink(
            &GridCell {
                style: GridStyle::INVERSE,
                ..cell('a')
            },
            &theme,
            &decode,
        );
        assert_eq!((swapped.fg, swapped.bg), (text.1, Some(text.0)));
        let selected = ink(
            &GridCell {
                style: GridStyle::SELECTED.with(GridStyle::INVERSE),
                ..red
            },
            &theme,
            &decode,
        );
        let region = (
            theme.foreground(Face::Region),
            theme.background(Face::Region),
        );
        assert_eq!((selected.fg, selected.bg), (region.0, Some(region.1)));
        let dim = ink(
            &GridCell {
                style: GridStyle::DIM,
                ..cell('a')
            },
            &theme,
            &decode,
        );
        for i in 0..3 {
            assert!((dim.fg[i] - (text.0[i] + text.1[i]) / 2.0).abs() < 1e-6);
        }
        assert_eq!(Decode::default().rgb([0, 128, 255])[2], 1.0);
    }

    #[test]
    fn a_style_holds_its_flags() {
        let style = GridStyle::BOLD.with(GridStyle::WIDE);
        assert!(style.contains(GridStyle::BOLD) && style.contains(GridStyle::WIDE));
        assert!(!style.contains(GridStyle::ITALIC));
        assert!(style.contains(GridStyle::default()));
        assert_eq!(style.without(GridStyle::WIDE), GridStyle::BOLD);
        let wide = GridCell {
            style: GridStyle::WIDE.with(GridStyle::GRAPHEME),
            ..cell('a')
        };
        assert_eq!(look(&wide), look(&cell('b')), "one run");
        let underlined = GridCell {
            style: GridStyle::UNDERLINE,
            ..cell('a')
        };
        assert_ne!(look(&underlined), look(&cell('a')));
    }
}
