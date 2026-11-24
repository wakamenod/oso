//! What a frame is made of: the list of panes the caller hands the renderer,
//! and the faces they draw with (ADR 0061).
//!
//! Must not depend on: `atlas`, `cells`, `grid`, `rows`, `scene`, `shape`, `text` or the
//! GPU. These are plain data, and everything else in this crate reads them.

use std::ops::Range;

use oso_core::buffer::BufferId;
use oso_core::display_map::{DisplayPoint, DisplaySnapshot, Preedit};
use oso_core::frame::CellRect;
use oso_core::syntax::Highlights;
use oso_core::window::Screen;

pub use face::Face;
pub(crate) use look::Look;

mod face;
mod highlights;
mod look;

/// Names a pane across frames, so that it keeps its own shaped rows. Each pane
/// in a list needs its own. A pane missing from a frame's list loses them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneId(pub u32);

/// How far forward a pane is. A higher layer hides what is under it. Panes in
/// one layer must not overlap: they are drawn together, all their rectangles,
/// then all their rounded boxes, then all their text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Layer(pub u8);

impl Layer {
    /// The regions of the frame's layout, which never overlap (ADR 0057).
    pub const TILED: Self = Self(0);
    /// A float's panel, over the tiled regions (ADR 0072, ADR 0075).
    pub const FLOAT: Self = Self(16);
    /// What a float shows, over its panel.
    pub const FLOAT_CONTENT: Self = Self(17);
}

/// The bands of ADR 0052 a pane's glyphs keep past its region: above its first row and below
/// its last. Only into cells nothing else draws on, such as a float's padding, so that a
/// glyph never draws over another pane or outside its float (ADR 0072).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bands {
    pub above: bool,
    pub below: bool,
}

impl Bands {
    /// None: a glyph stays in its pane, but for the rules of the frame's top row and of the
    /// modeline and the echo row (ADR 0061, ADR 0063).
    pub const NONE: Self = Self {
        above: false,
        below: false,
    };
}

/// Lines drawn inside a pane's region, each from its left edge to its right, a thin line's width
/// along it, in a face's foreground: along the top of its first row and the bottom of its last
/// (ADR 0170). They take no cell. They are over the background and a view's fills up to its current
/// line's band, and under the fills after it, its flash and its cursor. The pane's text, its boxes
/// and what an input method composes are cut to between them, and keep no band past them, so none
/// of it is drawn where a line is. A jump's labels, a server's marks, a grid, a divider and signs
/// are not cut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    pub above: Option<Face>,
    pub below: Option<Face>,
}

impl Rules {
    /// None.
    pub const NONE: Self = Self {
        above: None,
        below: None,
    };
}

/// How a view's cursor is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorStyle {
    /// A filled box, with the glyphs on it in its color: the selected window's.
    #[default]
    Box,
    /// The box's outline, over glyphs as they are: a window that is not selected, such as
    /// the one beneath a prompt (ADR 0072).
    Hollow,
    /// No cursor at all: a list that marks the row at point with a band instead, as
    /// `*Spaces*` in the side window (ADR 0131).
    None,
}

/// A window's view of a buffer.
pub struct TextView<'a> {
    /// The buffer's display model.
    pub display: DisplaySnapshot,
    /// The buffer, so that cached shaping of another one is not reused.
    pub buffer: BufferId,
    /// The buffer's revision: while it holds, cached rows are reused
    /// without looking at the buffer.
    pub revision: u64,
    /// The rows it shows from its top, as `Editor::screen` gives them: each part of a
    /// logical line, wrapped or not, with the mark on those that continue (ADR 0104).
    pub screen: &'a Screen,
    /// The primary cursor. The box goes here, or where `preedit` puts it.
    pub cursor: DisplayPoint,
    /// How the cursor is drawn.
    pub cursor_style: CursorStyle,
    /// The frame's cells of the other cursors' boxes, drawn as outlines in
    /// [`Face::SecondaryCursor`] over the text (ADR 0091), as `Editor::other_cursor_cells`
    /// gives them.
    pub others: &'a [CellRect],
    /// What an input method is composing, spliced into the cursor's row at the
    /// cursor and pushing the rest of it (ADR 0069).
    pub preedit: Option<&'a Preedit>,
    /// The frame's cells filled behind the text and its boxes, each list in its face's
    /// background, back to front: isearch's matches (ADR 0082), then the regions between
    /// marks and cursors (ADR 0071), as `Editor::isearch_cells` and `frame::region_cells`
    /// give them. A ruled pane's lines are just over the current line's band among them, or under
    /// them all with none ([`Rules`]).
    pub fills: &'a [(Face, &'a [CellRect])],
    /// Text drawn over the frame's cells after the text, as a jump's labels (ADR 0116): each
    /// its cells, one row and one cell a character, and its text and face, the face's
    /// background filling the cells. The text's glyphs under them are not drawn. The caller sorts
    /// them by row, then column, which drawing relies on: a row finds its own with a search.
    pub labels: &'a [(CellRect, Span<'a>)],
    /// Whether whitespace is marked (ADR 0097). A change shapes the rows in view again.
    pub whitespace: bool,
    /// The cursor flash, filled over the fills and under the text (ADR 0097).
    pub flash: Option<FlashFill>,
    /// What the buffer's text is, as highlighting last said, or `None` to draw it plain
    /// (ADR 0111). It must cover the buffer at `revision`. On the fills but the current line's
    /// band, and under the flash, text keeps [`Face::Text`]'s colour, which meets its floor
    /// there.
    pub highlights: Option<&'a Highlights>,
    /// Whether its highlights give its text bold, italic and underlines too: a native session's
    /// transcript (ADR 0165). No other view has any, and none looks for them.
    pub styled: bool,
    /// What a language server says of the text it shows (ADR 0138).
    pub marks: TextMarks<'a>,
}

/// What is drawn on a view's text for a language server's diagnostics (ADR 0138), each in the
/// frame's cells, as `Editor::diagnostic_cells` gives them. Hints are in none of them.
#[derive(Clone, Copy, Default)]
pub struct TextMarks<'a> {
    /// Cells with a wavy line under them, each list in its face's foreground: over the fills and
    /// under the glyphs, but in the cursor's cells. The lists should not meet, the most severe
    /// first, as it keeps its room if a frame runs out of it.
    pub underlines: &'a [(Face, &'a [CellRect])],
    /// Cells whose text is faded toward the background, its syntax colours kept: code the server
    /// says is unnecessary. On a fill other than the current line's band, and under the flash,
    /// the cursor box and a label, text is drawn as it is, which meets its floor there.
    pub faded: &'a [CellRect],
    /// Cells with a line through them, in the text's colour: code the server says is
    /// deprecated.
    pub struck: &'a [CellRect],
}

impl TextMarks<'_> {
    /// Nothing: a buffer no server follows.
    pub const NONE: Self = Self {
        underlines: &[],
        faded: &[],
        struck: &[],
    };
}

/// The cursor flash in a frame: its cells, as `Editor::flash_cells` gives them, filled in
/// [`Face::Flash`]'s background at `strength`, from 1 down to 0 (ADR 0097).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlashFill {
    pub cells: CellRect,
    pub strength: f32,
}

/// Text in one face's foreground, in cells after the span before it. Its
/// clusters come from the display model, so control and invisible characters
/// in it are escapes, and line breaks and tabs too (`text_clusters`). It is shaped
/// on its own, so no mark, ligature or join forms with the span before or after.
pub struct Span<'a> {
    pub text: &'a str,
    pub face: Face,
}

/// What a pane shows.
pub enum PaneContent<'a> {
    /// Rows of a buffer.
    Text(TextView<'a>),
    /// Rows that are no buffer's, from the pane's top row, each a list of spans
    /// from its first column. Each shown row is read and compared with what
    /// was shaped every frame, so they carry no revision. Large and long-lived
    /// text belongs in a buffer.
    Lines(&'a [&'a [Span<'a>]]),
    /// One row that stands for typed text without showing it, such as one `*` per
    /// character of a passphrase (ADR 0072), with a cursor box.
    Masked(Masked<'a>),
    /// The line between side by side windows: one pixel wide down the middle of the pane's
    /// column, in [`Face::Divider`] (ADR 0112).
    Divider,
    /// A terminal's cell grid (ADR 0120).
    Grid(GridView<'a>),
    /// A column of signs, one a row from the pane's top: a bar in the face's foreground, a third
    /// of a cell wide at its left and the row's full height, or nothing (ADR 0138). A row past
    /// the list has none.
    Signs(&'a [Option<Face>]),
}

/// What [`PaneContent::Grid`] shows: a terminal's visible rows, from the pane's top row, each
/// cell at its own column (ADR 0120). Rows past the pane and cells past its width are not
/// drawn, and what the rows leave of the pane shows the text's background.
pub struct GridView<'a> {
    /// Which grid it is, so that shaped rows of another are not reused.
    pub id: u64,
    pub rows: &'a [GridRow<'a>],
    /// The cursor, or `None` while the program hides it.
    pub cursor: Option<GridCursor>,
    /// What an input method is composing, drawn over the cells from `preedit_at` on, which it
    /// covers and does not push (ADR 0069).
    pub preedit: Option<&'a Preedit>,
    /// The row and column the preedit starts at: the program's cursor cell, also while the
    /// program hides the cursor, as a TUI's input box does (ADR 0120).
    pub preedit_at: (u16, u16),
}

/// One row of a [`GridView`]. No `Debug`: it holds what a program printed (ADR 0028).
pub struct GridRow<'a> {
    /// Changes whenever the row's cells do. While it holds, the row is not read again.
    pub generation: u64,
    /// From the row's first column, one a column.
    pub cells: &'a [GridCell],
    /// The text of each cell of more than one code point, one after another.
    pub text: &'a str,
    /// Each such cell's column and its bytes in `text`, sorted by column.
    pub graphemes: &'a [(u16, Range<u32>)],
}

/// One cell of a [`GridRow`]. A cell of [`GridStyle::GRAPHEME`] takes its text from the row's
/// `graphemes`, and any other `ch`. `' '` and `'\0'` draw no glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridCell {
    pub ch: char,
    pub fg: GridColor,
    pub bg: GridColor,
    pub underline_color: GridColor,
    pub style: GridStyle,
}

/// A cell's colour, resolved when it is drawn, so a switch of theme needs no new rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridColor {
    /// [`Face::Text`]'s foreground for a cell's text, its background for the cell's
    /// background, and the cell's text's colour for its underline.
    Default,
    /// The face's foreground, for the text or the background alike: the terminal faces are
    /// single colours.
    Face(Face),
    /// sRGB, drawn as given.
    Rgb([u8; 3]),
}

/// How a cell is drawn, as flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GridStyle(u16);

impl GridStyle {
    /// Kept, and drawn regular (ADR 0120).
    pub const BOLD: Self = Self(1);
    /// Kept, and drawn regular.
    pub const ITALIC: Self = Self(1 << 1);
    /// The text's colour halfway to the background's.
    pub const DIM: Self = Self(1 << 2);
    /// The text's and the background's colours swapped.
    pub const INVERSE: Self = Self(1 << 3);
    /// No glyph, only the background.
    pub const HIDDEN: Self = Self(1 << 4);
    /// A line through the cell.
    pub const STRIKE: Self = Self(1 << 5);
    /// A line under the cell, in its underline colour.
    pub const UNDERLINE: Self = Self(1 << 6);
    /// Two lines under the cell.
    pub const DOUBLE_UNDERLINE: Self = Self(1 << 7);
    /// A glyph two columns wide, over this cell and the spacer after it.
    pub const WIDE: Self = Self(1 << 8);
    /// The second column of a wide cell: only its background is drawn.
    pub const WIDE_SPACER: Self = Self(1 << 9);
    /// The cell's text is more than one code point, in the row's `graphemes`.
    pub const GRAPHEME: Self = Self(1 << 10);
    /// Selected with the mouse: drawn in [`Face::Region`]'s colours, whatever the cell's
    /// own (ADR 0120, step 4b-2).
    pub const SELECTED: Self = Self(1 << 11);

    /// Whether every flag of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The flags of both.
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The flags of this one but `other`'s.
    pub(crate) const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

/// A terminal's cursor, in its grid's cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridCursor {
    pub row: u16,
    pub column: u16,
    pub shape: GridCursorShape,
    /// Drawn as the outline of its cell, whatever its shape, as a window that is not
    /// selected draws its cursor.
    pub hollow: bool,
}

/// How a program asks its cursor to look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridCursorShape {
    /// The cursor face's box, with the glyph on it in the box's own colour.
    Block,
    /// A thin bar at the cell's left.
    Bar,
    /// A line at the cell's bottom.
    Underline,
}

/// What [`PaneContent::Masked`] shows.
pub struct Masked<'a> {
    /// What is drawn from the pane's first column, in its place.
    pub text: &'a str,
    /// The column of the cursor box, from the pane's first.
    pub cursor: usize,
}

/// A float's panel (ADR 0075), drawn in pixels around cells, with its bottom corners rounded,
/// a hairline in [`Face::FloatBorder`] and a soft shadow, all cut to its pane's region. On the
/// frame's top row it also covers the margin above, up to the surface's edge, where its
/// corners stay square.
pub struct Panel<'a> {
    /// The cells it covers, in [`Face::Float`].
    pub cells: CellRect,
    /// Cells drawn as an input field, a little larger than they are, in [`Face::Field`]: its
    /// fill in the background and its outline in the foreground.
    pub field: Option<CellRect>,
    /// The cells the field may reach into past its own, such as the padding around them. Its
    /// box, outline included, is cut to them.
    pub field_room: CellRect,
    /// Cells drawn as key chips, each in its face's background: ` y ` and ` n ` in
    /// [`Face::Key`], inside their cells, and a menu's keys in [`Face::MenuKey`],
    /// [`Face::MenuKeyStay`] or [`Face::MenuKeySwitch`], reaching a little past theirs, as their
    /// cells hold only the key (ADR 0117).
    pub keys: &'a [(CellRect, Face)],
    /// A row of cells drawn as the selected candidate's box, a little wider than they are,
    /// in [`Face::CompletionSelected`] (ADR 0076).
    pub selected: Option<CellRect>,
    /// A row of cells marked as a menu's default head, the one `RET` runs: a box a little wider
    /// than they are, in [`Face::CompletionSelected`], never stretched across the panel
    /// (ADR 0117).
    pub marked: Option<CellRect>,
    /// A completion's float (ADR 0076): filled with the text's background and framed by a
    /// border in place of the hairline, with square corners and no shadow, and its selected row
    /// a square bar on the cells `selected` gives: across the panel, or beside a preview only
    /// as far as the column that divides them (ADR 0088).
    pub border: bool,
}

/// A region of the frame and what it shows. Nothing of it is drawn outside the
/// region, and nothing of what is under it shows through its background. The
/// exceptions are the frame's top row, whose glyphs keep the band above their row
/// in the top margin (ADR 0052), and a tiled pane of one row of [`PaneContent::Lines`], whose
/// glyphs keep it on the row above, over what that row shows (ADR 0063). A pane in
/// front on the top row does not cover the margin (ADR 0061), but for a float's
/// [`Panel`], which covers it up to the surface's edge (ADR 0075).
///
/// A frame's list needs no order between layers. Inside a layer, later panes come
/// first if a frame runs out of room for glyphs or rectangles, so what must always
/// show, such as a modeline, goes after a window's text.
pub struct Pane<'a> {
    pub id: PaneId,
    /// The frame's cells it is drawn in. It may be empty.
    pub region: CellRect,
    pub layer: Layer,
    /// Filled in behind the content, in this face's background. `None` leaves
    /// what is under the pane, so a pane above others needs one to hide them.
    pub background: Option<Face>,
    /// A float's panel, drawn over the background and under the content (ADR 0075).
    pub panel: Option<Panel<'a>>,
    /// The bands its glyphs keep past its region, but past a line of its `rules`.
    pub bands: Bands,
    /// The lines along its edges.
    pub rules: Rules,
    pub content: PaneContent<'a>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_order_from_the_back() {
        assert!(Layer::TILED < Layer(1));
        assert!(Layer::TILED < Layer::FLOAT && Layer::FLOAT < Layer::FLOAT_CONTENT);
    }
}
