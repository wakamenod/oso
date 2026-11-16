//! Completion at point (ADR 0138, Completion in the buffer, step 9), on the UI thread: the ask
//! `completion-at-point` makes, sent after the event's edits went to the server, and its
//! answer, which opens the popup while nothing moved since the ask.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.
//!
//! - An answer for another revision, cursor, window or buffer, or once a prompt opened, is
//!   dropped: the editor judges it, as it opens the popup.
//! - A row shows its label, then its kind's word and its detail's first line, its runs of
//!   blanks as one, so that no server's text passes as oso's own.
//! - The answer's items go to the freeing thread: 5000 are many to free on the UI thread.

use oso_core::editor::{PopupAnswer, PopupItem};
use oso_lsp::protocol::kind_name;
use oso_lsp::server::Completions;

use super::super::App;
use super::places::Asker;

impl App {
    /// Opens the popup for `completions`, which server `index` answered to `asker`'s ask
    /// about `asked`, unless the ask is stale.
    pub(super) fn completed(
        &mut self,
        (asked, asker): (oso_core::buffer::BufferId, Asker),
        completions: Completions,
    ) {
        if !self.still_there(asked, &asker) {
            self.freer.free(completions);
            return;
        }
        let Completions {
            revision,
            items,
            incomplete: _,
        } = completions;
        let items = items
            .into_iter()
            .map(|item| {
                let detail = item.detail.lines().next().unwrap_or_default();
                let words = item.kind.map(kind_name).into_iter().chain([detail]);
                let annotation = words
                    .flat_map(str::split_whitespace)
                    .collect::<Vec<_>>()
                    .join(" ");
                PopupItem {
                    label: item.label.into(),
                    annotation: annotation.into(),
                    filter: item.filter.map(Into::into),
                    text: item.text.into(),
                    range: item.range,
                }
            })
            .collect();
        let answer = PopupAnswer {
            at: self.editor.ask_at(),
            revision,
            items,
        };
        if let Err(Some(why)) = self.editor.open_popup(answer) {
            self.editor.message(why);
        }
        self.request_redraw();
    }
}
