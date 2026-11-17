//! The `list` of a prompt in the agent's `state` (ADR 0138, step 15): the person's open list of
//! a server's places or of a project's diagnostics, as a frame shows it, so that an agent can
//! look at what the person sees.
//!
//! Must not depend on: sockets, threads or `app`; this is a plain function of the editor.
//!
//! - Nothing of it while a buffer that may hold secrets is shown; never the person's input
//!   itself, though the rows shown follow it.
//! - A row of a file that may hold secrets, or of a buffer the channel may not read or is still
//!   opening, gives its path and line only, and the preview of one is withheld.
//! - The rows' paths and texts are written as a server's text is (`server_text`); the
//!   preview's rows are the text the frame draws, as `text` gives a buffer's.

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{Editor, ListedOf, ListedPreview};
use serde_json::{Value, json};

use super::unreadable;
use crate::channel::diagnostics::server_text;

/// The `list` of the innermost prompt, if it is such a list and may be shown; `server` names
/// the server a list of places is of.
pub(super) fn list_state(editor: &Editor, server: Option<&str>) -> Option<Value> {
    if editor.shows_secret() {
        return None;
    }
    let view = editor.listed_view()?;
    let closed = |id: BufferId| {
        (editor.buffer(id)).is_none_or(|buffer| unreadable(buffer) || buffer.agent_pending())
    };
    let mut selected_closed = false;
    let rows: Vec<Value> = (view.rows.iter().enumerate())
        .map(|(index, row)| {
            let hidden = Buffer::name_may_hold_secrets(&row.path) || row.buffer.is_some_and(closed);
            if hidden && view.selected == Some(index) {
                selected_closed = true;
            }
            json!({
                "path": server_text(row.shown, false),
                "line": row.line,
                "text": (!hidden).then(|| server_text(row.text, false)),
            })
        })
        .collect();
    let withheld = json!({ "kind": "withheld" });
    let preview = match view.preview {
        _ if selected_closed => withheld,
        ListedPreview::Nothing => Value::Null,
        ListedPreview::Withheld => withheld,
        ListedPreview::Rows { buffer, .. } if buffer.is_some_and(closed) => withheld,
        ListedPreview::Rows { buffer, at, rows } => json!({
            "kind": if buffer.is_some() { "buffer" } else { "file" },
            "at": at,
            "rows": rows
                .into_iter()
                .map(|(line, text)| json!({ "line": line, "text": text }))
                .collect::<Vec<_>>(),
        }),
    };
    let (of, server) = match view.of {
        ListedOf::Definitions => ("definitions", server),
        ListedOf::References => ("references", server),
        // A project's diagnostics are every server's.
        ListedOf::Diagnostics => ("diagnostics", None),
    };
    Some(json!({
        "of": of,
        "server": server,
        "count": view.count,
        "total": view.total,
        "cut": view.cut,
        "pending": view.pending,
        "selected": view.selected,
        "rows": rows,
        "preview": preview,
    }))
}

#[cfg(test)]
mod tests;
