//! The font style a face's text is shaped in, and whether a line is drawn under it (ADR 0165,
//! "The look: styles"): a native session's own classes alone have any.
//!
//! Must not depend on: `atlas`, `cells`, `grid`, `rows`, `scene`, `shape`, `text` or the
//! GPU, as the rest of the pane's types.

use oso_core::syntax::Highlight;
use oso_platform::text::FontStyle;

use super::Face;

/// How highlighted text is drawn besides its colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Look {
    pub(crate) style: FontStyle,
    pub(crate) underlined: bool,
}

impl Look {
    /// The look of text that is `highlight`. The same in every theme.
    pub(crate) fn of(highlight: Option<Highlight>) -> Self {
        highlight.map_or(Self::default(), |highlight| {
            Self::of_face(Face::of_highlight(highlight))
        })
    }

    /// The look of text in `face`. A test over every face says which have one.
    pub(crate) fn of_face(face: Face) -> Self {
        let style = match face {
            Face::SessionHeading | Face::SessionStrong | Face::SessionStrongCode => FontStyle::Bold,
            Face::SessionEmphasis | Face::SessionThinking => FontStyle::Italic,
            _ => FontStyle::Regular,
        };
        Self {
            style,
            underlined: face == Face::SessionLink,
        }
    }
}
