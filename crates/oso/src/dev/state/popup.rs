//! The completion popup in `get-state` (ADR 0138, step 9): its rows, for end-to-end flows to
//! wait on.
//!
//! Must not depend on: `app`, winit or sockets.

use oso_core::editor::Editor;
use serde_json::{Value, json};

/// The completion popup's rows, the selected one, the matches, whether it moved (ADR 0138);
/// only that it is open while a buffer that may hold secrets shows. The agent's has none.
pub(super) fn popup_state(editor: &Editor) -> Value {
    editor.popup().map_or(Value::Null, |popup| {
        if editor.shows_secret() {
            return json!({ "open": true });
        }
        json!({
            "rows": popup.rows().map(|(_, row)| row.text()).collect::<Vec<_>>(),
            "selected": popup.rows().position(|(selected, _)| selected),
            "count": popup.len(),
            "moved": popup.moved(),
        })
    })
}
