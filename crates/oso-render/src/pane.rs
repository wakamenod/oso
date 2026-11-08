//! What a frame is made of: the list of panes the caller hands the renderer,
//! and the faces they draw with (ADR 0061).
//!
//! Must not depend on: `atlas`, `grid`, `rows`, `scene`, `shape`, `text` or the
//! GPU. These are plain data, and everything else in this crate reads them.

use oso_core::buffer::BufferId;
use oso_core::display_map::{CellStyle, DisplayPoint, DisplaySnapshot, Preedit};
use oso_core::frame::CellRect;
use oso_core::window::Scroll;

// Colors in linear RGB, until the theme of ADR 0027.
const BACKGROUND: [f32; 4] = [0.05, 0.05, 0.07, 1.0];
const FOREGROUND: [f32; 4] = [0.90, 0.90, 0.92, 1.0];
const CURSOR: [f32; 4] = [0.90, 0.90, 0.92, 1.0];
const ESCAPE: [f32; 4] = [0.85, 0.45, 0.20, 1.0];
const ESCAPE_BOX: [f32; 4] = [0.22, 0.10, 0.05, 1.0];
const MODELINE: [f32; 4] = [0.16, 0.16, 0.21, 1.0];
const WARNING: [f32; 4] = [1.0, 0.72, 0.16, 1.0];
// A float's panel (ADR 0075): a fill only a little lighter than the text's, a hairline of
// white at low alpha, a dim label, a field on the text's background with an accent outline,
// and key chips.
const FLOAT: [f32; 4] = [0.063, 0.0666, 0.0999, 1.0];
const FLOAT_HAIRLINE: [f32; 4] = [1.0, 1.0, 1.0, 0.035];
// The label 5.3:1 on the fill and the key 5.6:1 on its chip, against ADR 0027's 4.5:1 at
// least, and a cool tint apart from the text's.
const PROMPT: [f32; 4] = [0.52, 0.57, 0.74, 1.0];
const FIELD_OUTLINE: [f32; 4] = [0.159, 0.2384, 0.5089, 1.0];
const KEY: [f32; 4] = [0.0908, 0.0953, 0.1413, 1.0];
const KEY_TEXT: [f32; 4] = [0.73, 0.77, 0.88, 1.0];
const REGION: [f32; 4] = [0.06, 0.10, 0.20, 1.0];
// isearch's matches (ADR 0082): a muted amber for the others in view, and a strong one for
// the current match, behind text that keeps its colour, until the themes (M2-23). The text
// stays 4.6:1 on the strong one and 8:1 on the muted one, against ADR 0027's 4.5:1.
const MATCH: [f32; 4] = [0.12, 0.05, 0.005, 1.0];
const CURRENT_MATCH: [f32; 4] = [0.32, 0.12, 0.004, 1.0];

/// How a float's panel is drawn, beyond its faces' colors: sizes in points (the cell's height
/// over 16) and the shadow's color and strength (ADR 0075). It is read every frame, as the
/// faces are, so the theme (M2-23) can switch it at run time: a light theme wants a lighter,
/// smaller shadow and a dark hairline.
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
}

/// The panel's style until the theme.
pub(crate) const PANEL: PanelStyle = PanelStyle {
    radius: 8.0,
    hairline: 1.0,
    shadow: [0.0024, 0.0027, 0.0044, 0.55],
    shadow_down: 10.0,
    shadow_sigma: 14.0,
    field_reach: [6.0, 4.0],
    field_radius: 5.0,
    field_outline: 1.0,
    key_inset: [1.0, 2.0],
    key_radius: 4.0,
};

/// What a piece of the screen means. The renderer turns it into colors, and
/// the theme (M2-23) will map it to its palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// Text, on the background behind everything else.
    Text,
    /// The label of an escape (`^M`, `\xFF`), not to be mistaken for text
    /// that reads the same.
    Escape,
    /// The label of a boxed code point (`U+202E`), and its box. Also a cluster
    /// the renderer shapes only in part, past its budget of characters.
    Boxed,
    /// The cursor box, and the glyphs on it.
    Cursor,
    /// A modeline, and its text.
    Modeline,
    /// A warning that must be seen, on whatever is behind it.
    Warning,
    /// Text an input method is composing, and its underline (ADR 0069). It is
    /// shaped apart from the text around it.
    Preedit,
    /// A float's text, on its own background (ADR 0072).
    Float,
    /// The hairline around a float's panel (ADR 0075).
    FloatBorder,
    /// What a prompt says before its answer, dim on the float's background.
    Prompt,
    /// An input field in a float: the text's background, and its outline in the foreground
    /// (ADR 0075).
    Field,
    /// A key's chip, such as a question's `y`, and the key on it.
    Key,
    /// The region between a mark and its cursor (ADR 0071). Only its background
    /// is drawn, behind the text, which keeps its own face.
    Region,
    /// An isearch match in view, drawn as the region is (ADR 0082).
    SearchMatch,
    /// The current isearch match, which stands out from the others.
    SearchCurrent,
}

impl Face {
    /// The color of coverage glyphs.
    pub(crate) fn foreground(self) -> [f32; 4] {
        match self {
            Self::Text
            | Self::Modeline
            | Self::Preedit
            | Self::Float
            | Self::Region
            | Self::SearchMatch
            | Self::SearchCurrent => FOREGROUND,
            Self::Escape | Self::Boxed => ESCAPE,
            Self::Cursor => BACKGROUND,
            Self::Warning => WARNING,
            Self::FloatBorder => FLOAT_HAIRLINE,
            Self::Prompt => PROMPT,
            Self::Field => FIELD_OUTLINE,
            Self::Key => KEY_TEXT,
        }
    }

    /// The color of rectangles.
    pub(crate) fn background(self) -> [f32; 4] {
        match self {
            Self::Text | Self::Escape | Self::Warning | Self::Field => BACKGROUND,
            Self::Boxed => ESCAPE_BOX,
            Self::Cursor => CURSOR,
            Self::Modeline => MODELINE,
            Self::Preedit => FOREGROUND,
            Self::Float | Self::Prompt => FLOAT,
            Self::FloatBorder => FLOAT_HAIRLINE,
            Self::Key => KEY,
            Self::Region => REGION,
            Self::SearchMatch => MATCH,
            Self::SearchCurrent => CURRENT_MATCH,
        }
    }

    /// The face of a run in `style` within text of this face. An escape looks
    /// like an escape wherever it is.
    pub(crate) fn of_run(self, style: CellStyle) -> Self {
        match style {
            CellStyle::Text => self,
            CellStyle::Escape => Self::Escape,
            CellStyle::Boxed => Self::Boxed,
        }
    }
}

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

/// How a view's cursor is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorStyle {
    /// A filled box, with the glyphs on it in its color: the selected window's.
    #[default]
    Box,
    /// The box's outline, over glyphs as they are: a window that is not selected, such as
    /// the one beneath a prompt (ADR 0072).
    Hollow,
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
    /// The display row and column at the top left.
    pub scroll: Scroll,
    /// The primary cursor. The box goes here, or where `preedit` puts it.
    pub cursor: DisplayPoint,
    /// How the cursor is drawn.
    pub cursor_style: CursorStyle,
    /// What an input method is composing, spliced into the cursor's row at the
    /// cursor and pushing the rest of it (ADR 0069).
    pub preedit: Option<&'a Preedit>,
    /// The frame's cells filled behind the text and its boxes, each list in its face's
    /// background, back to front: isearch's matches (ADR 0082), then the regions between
    /// marks and cursors (ADR 0071), as `Editor::isearch_cells` and `frame::region_cells`
    /// give them.
    pub fills: &'a [(Face, &'a [CellRect])],
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
    /// Cells drawn as key chips, such as ` y ` and ` n `, in [`Face::Key`].
    pub keys: &'a [CellRect],
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
    /// The bands its glyphs keep past its region.
    pub bands: Bands,
    pub content: PaneContent<'a>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_keeps_its_face_inside_text_of_another() {
        assert_eq!(Face::Cursor.of_run(CellStyle::Text), Face::Cursor);
        assert_eq!(Face::Cursor.of_run(CellStyle::Escape), Face::Escape);
        assert_eq!(Face::Cursor.of_run(CellStyle::Boxed), Face::Boxed);
    }

    #[test]
    fn layers_order_from_the_back() {
        assert!(Layer::TILED < Layer(1));
        assert!(Layer::TILED < Layer::FLOAT && Layer::FLOAT < Layer::FLOAT_CONTENT);
    }
}
