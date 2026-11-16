//! Completion at point (ADR 0138, Completion in the buffer, step 9), on the UI thread: the ask
//! `completion-at-point` makes, sent after the event's edits went to the server, and its
//! answer, which opens the popup while nothing moved since the ask.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.
//!
//! - An answer for another revision, cursor, window or buffer, or once a prompt opened, is
//!   dropped: the editor judges it, as it opens the popup.
//! - A row shows its label, then its kind's word and its detail's first line, which the editor
//!   cuts and makes the runs of blanks of one.
//! - A stale answer goes to the freeing thread; the editor frees one it drops, and a popup's
//!   items once it closes, on its worker.

use oso_core::editor::{PopupAnswer, PopupItem};
use oso_lsp::protocol::kind_name;
use oso_lsp::server::Completions;

use super::super::App;

/// The most bytes of a detail an annotation copies: more than a row shows.
const DETAIL_SHOWN: usize = 256;
use super::asks::Asker;

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
                // The kind's word and the detail's first line, at most what a row shows; the
                // editor makes its runs of blanks one.
                let detail = item.detail.lines().next().unwrap_or_default();
                let detail = &detail[..detail.floor_char_boundary(DETAIL_SHOWN)];
                let mut annotation = String::with_capacity(16 + detail.len());
                annotation.push_str(item.kind.map_or("", kind_name));
                annotation.push(' ');
                annotation.push_str(detail);
                PopupItem {
                    label: item.label.into(),
                    annotation: annotation.into(),
                    filter: item.filter.map(Into::into),
                    text: item.text.into(),
                    range: item.range,
                }
            })
            .collect();
        // Where it was asked: the editor judges the answer against it too.
        let answer = PopupAnswer {
            at: asker.at(asked),
            revision,
            items,
        };
        if let Err(Some(why)) = self.editor.open_popup(answer) {
            self.editor.message(why);
        }
        self.request_redraw();
    }
}
