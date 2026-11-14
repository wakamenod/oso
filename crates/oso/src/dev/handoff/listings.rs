//! Internal buffers in the state a restart hands over: where a window on one goes back to
//! (ADR 0051), and the directory editor's listings, which come back themselves, each by its
//! directory, its order, whether it shows details and its cursor's line, listed again
//! (ADR 0134).
//!
//! Must not depend on: `app`, `server`, winit or sockets.

use oso_core::buffer::BufferId;
use oso_core::editor::{Editor, Listing};
use oso_core::selection::SelectionSet;
use oso_core::window::{Scroll, Window};
use serde_json::{Value, json};

/// Every listing's entry in the state's buffers.
pub(super) fn states(editor: &Editor) -> Vec<Value> {
    editor
        .listings()
        .map(|(id, listing)| state(editor, id, listing))
        .collect()
}

/// A directory listing as the state keeps it (ADR 0134): its directory, its order, whether it
/// shows details, and the line of the first window's cursor on it, which the new instance
/// lists again. One whose directory's path is not UTF-8 comes back as `*scratch*`.
fn state(editor: &Editor, id: BufferId, listing: &Listing) -> Value {
    let Some(directory) = listing.directory.to_str() else {
        return json!({ "id": id.get(), "path": null });
    };
    let line = editor
        .listing_points(id)
        .first()
        .map_or(0, |(_, line)| *line);
    json!({
        "id": id.get(),
        "path": null,
        "listing": {
            "directory": directory,
            "details": listing.details,
            "by_date": listing.note == "by date",
            "line": line,
        },
    })
}

/// Where `window` goes back to, if it is on an internal buffer and has somewhere to go that
/// cannot hold secrets.
pub(super) fn behind_internal<'a>(
    editor: &'a Editor,
    window: &Window,
) -> Option<(BufferId, &'a SelectionSet, Scroll)> {
    // One that may hold secrets is not shown again by a restart: the window comes back on
    // `*scratch*`, and the buffer is handed over unshown.
    // A directory listing comes back itself (ADR 0134).
    let internal = editor.shown(window).is_internal() && editor.listing(window.buffer()).is_none();
    internal
        .then(|| editor.place_behind_internal_of(window.id()))
        .flatten()
        .filter(|(buffer, ..)| {
            editor
                .buffer(*buffer)
                .is_some_and(|buffer| !buffer.is_secret())
        })
}

/// What `window` hands over: what it shows, or from an internal buffer, where it goes back
/// to, since no key could reach that again otherwise.
pub(super) fn handed_over<'a>(
    editor: &'a Editor,
    window: &'a Window,
) -> (BufferId, &'a SelectionSet, Scroll) {
    behind_internal(editor, window).unwrap_or((
        window.buffer(),
        &window.selections,
        window.scroll(),
    ))
}
