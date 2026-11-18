//! The preview of a search of conversations' row (ADR 0145): what the host is asked for, and
//! what is shown once it gives the excerpt, prose wrapped at the preview's width
//! ([`Excerpt::wrapped`](crate::editor::Excerpt::wrapped)), as a conversation has no file to
//! show from.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host makes the
//! text from what it holds and builds the excerpt on its own thread.

use crate::editor::project_search::{ConversationPreviewRequest, To};
use crate::editor::{Editor, Preview};

impl Editor {
    /// The text the host should give for the selected row's preview, once per row selected,
    /// if the innermost prompt searches conversations (ADR 0145), and not while the float has
    /// no room for a preview.
    pub fn take_conversation_preview_request(&mut self) -> Option<ConversationPreviewRequest> {
        self.project_search.source.conversations()?;
        let key = self.selected_search_row()?;
        if self.project_search.preview.asked() == Some(key) {
            return None;
        }
        let region = self
            .prompt_layout()
            .map(|layout| layout.regions.preview)
            .filter(|region| region.rows > 0 && region.columns > 0)?;
        let place = self.project_search.places.get(key.1)?;
        let To::Message { session, message } = &place.to else {
            return None;
        };
        let (session, message) = (session.clone(), *message);
        let (line, column) = (place.line, place.column);
        let (generation, live) = self.project_search.preview.ask(key);
        Some(ConversationPreviewRequest {
            generation,
            session,
            message,
            line,
            column,
            rows: region.rows,
            columns: region.columns,
            live,
        })
    }

    /// What the preview of the selected row shows, if it is a conversation's: the excerpt the
    /// host gave for it, once it came.
    pub(in crate::editor) fn conversation_preview(&self) -> Option<Preview> {
        let key = self.selected_search_row()?;
        let place = self.project_search.places.get(key.1)?;
        matches!(place.to, To::Message { .. })
            .then(|| self.project_search.preview.shown(key))
            .flatten()
    }
}
