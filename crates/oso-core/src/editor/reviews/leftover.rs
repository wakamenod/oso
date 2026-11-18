//! What putting in a review's text, paint or panes leaves over (ADR 0122): the rows,
//! highlights and texts replaced, held for the host to free off the UI thread.
//!
//! Must not depend on: `command`, `keymap` or the host's threads.

use crate::load::Reloaded;
use crate::review::Rows;
use crate::syntax::Highlights;

/// What putting in a review's text or paint left over: the rows, highlights and text it
/// replaced or did not take. They may be large, so the host frees them off the UI thread.
#[must_use = "free it off the UI thread"]
pub struct ReviewLeftover {
    pub(super) rows: Option<Rows>,
    pub(super) highlights: Option<Highlights>,
    pub(super) text: Option<Box<Reloaded>>,
    /// What a pane's text left over, for the two panes.
    pub(super) panes: Vec<ReviewLeftover>,
    /// The rows of panes dropped since the last leftover.
    pub(super) spare: Vec<Rows>,
}

impl ReviewLeftover {
    /// Nothing left over yet.
    pub(super) fn none() -> ReviewLeftover {
        ReviewLeftover {
            rows: None,
            highlights: None,
            text: None,
            panes: Vec::new(),
            spare: Vec::new(),
        }
    }

    /// A text that was not taken, alone.
    pub(in crate::editor) fn of_text(text: Reloaded) -> ReviewLeftover {
        ReviewLeftover {
            text: Some(Box::new(text)),
            ..ReviewLeftover::none()
        }
    }

    pub(super) fn of(rows: Rows, highlights: Highlights, text: Option<Reloaded>) -> ReviewLeftover {
        ReviewLeftover {
            rows: Some(rows),
            highlights: Some(highlights),
            text: text.map(Box::new),
            panes: Vec::new(),
            spare: Vec::new(),
        }
    }
}

#[cfg(test)]
impl ReviewLeftover {
    /// Whether it holds rows, highlights and a text.
    pub(crate) fn holds(&self) -> (bool, bool, bool) {
        (
            self.rows.is_some(),
            self.highlights.is_some(),
            self.text.is_some(),
        )
    }
}

impl std::fmt::Debug for ReviewLeftover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewLeftover")
            .field("rows", &self.rows.is_some())
            .field("highlights", &self.highlights.is_some())
            .field("text", &self.text.is_some())
            .field("panes", &self.panes)
            .field("spare", &self.spare.len())
            .finish()
    }
}
