//! What a frame is made of: the list of panes the caller hands the renderer,
//! and the faces they draw with (ADR 0061).
//!
//! Must not depend on: `atlas`, `grid`, `rows`, `scene`, `shape`, `text` or the
//! GPU. These are plain data, and everything else in this crate reads them.

use oso_core::buffer::BufferId;
use oso_core::display_map::{CellStyle, DisplayPoint, DisplaySnapshot, Preedit};
use oso_core::frame::CellRect;
use oso_core::window::Scroll;

/// What a piece of the screen means. The current theme gives it its colours (ADR 0089).
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
    /// The outline of a cursor other than the primary, dimmer than the cursor box, over
    /// glyphs as they are (ADR 0091). Only its background is drawn.
    SecondaryCursor,
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
    /// What a question asks, in a colour no buffer text is drawn in (ADR 0089).
    Question,
    /// An input field in a float: the text's background, and its outline in the foreground
    /// (ADR 0075).
    Field,
    /// A key's chip, such as a question's `y`, and the key on it.
    Key,
    /// The region between a mark and its cursor (ADR 0071). Only its background
    /// is drawn, behind the text, which keeps its own face.
    Region,
    /// The selected candidate's row in a float: its box's background, behind the text, which
    /// keeps its own faces (ADR 0076).
    CompletionSelected,
    /// What the input matched in a candidate.
    CompletionMatch,
    /// What is said of a candidate at the end of its row, and a list's count.
    CompletionAnnotation,
    /// A key sequence after a candidate, such as a command's binding.
    CompletionBinding,
    /// A completion's count and label, at the start of its answer's row.
    CompletionPrompt,
    /// An isearch match in view, drawn as the region is (ADR 0082).
    SearchMatch,
    /// The current isearch match, which stands out from the others.
    SearchCurrent,
}

impl Face {
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
    /// A row of cells drawn as the selected candidate's box, a little wider than they are,
    /// in [`Face::CompletionSelected`] (ADR 0076).
    pub selected: Option<CellRect>,
    /// A completion's float (ADR 0076): filled with the text's background and framed by a
    /// border in place of the hairline, with square corners and no shadow, and its selected row
    /// a bar across the whole panel.
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
