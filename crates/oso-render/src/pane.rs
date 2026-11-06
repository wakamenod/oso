//! What a frame is made of: the list of panes the caller hands the renderer,
//! and the faces they draw with (ADR 0061).
//!
//! Must not depend on: `atlas`, `grid`, `rows`, `scene`, `shape`, `text` or the
//! GPU. These are plain data, and everything else in this crate reads them.

use oso_core::buffer::BufferId;
use oso_core::display_map::{CellStyle, DisplayPoint, DisplaySnapshot};
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

/// What a piece of the screen means. The renderer turns it into colors, and
/// the theme (M2-23) will map it to its palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// Text, on the background behind everything else.
    Text,
    /// The label of an escape (`^M`, `\xFF`), not to be mistaken for text
    /// that reads the same.
    Escape,
    /// The label of a boxed code point (`U+202E`), and its box.
    Boxed,
    /// The cursor box, and the glyphs on it.
    Cursor,
    /// A modeline, and its text.
    Modeline,
    /// A warning that must be seen, on whatever is behind it.
    Warning,
}

impl Face {
    /// The color of coverage glyphs.
    pub(crate) fn foreground(self) -> [f32; 4] {
        match self {
            Self::Text | Self::Modeline => FOREGROUND,
            Self::Escape | Self::Boxed => ESCAPE,
            Self::Cursor => BACKGROUND,
            Self::Warning => WARNING,
        }
    }

    /// The color of rectangles.
    pub(crate) fn background(self) -> [f32; 4] {
        match self {
            Self::Text | Self::Escape | Self::Warning => BACKGROUND,
            Self::Boxed => ESCAPE_BOX,
            Self::Cursor => CURSOR,
            Self::Modeline => MODELINE,
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
/// one layer must not overlap: they are drawn together, all their rectangles
/// and then all their text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Layer(pub u8);

impl Layer {
    /// The regions of the frame's layout, which never overlap (ADR 0057).
    pub const TILED: Self = Self(0);
}

/// A window's view of a buffer.
pub struct TextView {
    /// The buffer's display model.
    pub display: DisplaySnapshot,
    /// The buffer, so that cached shaping of another one is not reused.
    pub buffer: BufferId,
    /// The buffer's revision: while it holds, cached rows are reused
    /// without looking at the buffer.
    pub revision: u64,
    /// The display row and column at the top left.
    pub scroll: Scroll,
    /// Where the cursor box goes.
    pub cursor: DisplayPoint,
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
    Text(TextView),
    /// Rows that are no buffer's, from the pane's top row, each a list of spans
    /// from its first column. Each shown row is read and compared with what
    /// was shaped every frame, so they carry no revision. Large and long-lived
    /// text belongs in a buffer.
    Lines(&'a [&'a [Span<'a>]]),
}

/// A region of the frame and what it shows. Nothing of it is drawn outside the
/// region, and nothing of what is under it shows through its background. The
/// exceptions are the frame's top row, whose glyphs keep the band above their row
/// in the top margin (ADR 0052), and a tiled row of [`PaneContent::Lines`], whose
/// glyphs keep it on the row above, over what that row shows (ADR 0063). A pane in
/// front on the top row does not cover the margin (ADR 0061).
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
    }
}
