//! What the dev socket's state says of agent sessions (ADR 0121 step 3), for end-to-end flows:
//! each session's state and recap, whether `*Sessions*` shows and its lines, the marks, and
//! how many notices asked for attention. The dev socket trusts the same user (ADR 0044); the
//! agent channel reads none of this.
//!
//! Must not depend on: how a frame is drawn.

use std::time::Instant;

use serde_json::json;

use crate::app::App;

impl App {
    /// Agent sessions as the dev socket's state gives them.
    pub(in crate::app) fn sessions_state(&self) -> serde_json::Value {
        let now = Instant::now();
        let sessions: Vec<_> = self
            .session_entries(now)
            .into_iter()
            .map(|entry| {
                let key = match entry.key {
                    crate::sessions::list::Key::Hosted(key) => json!({ "hosted": key }),
                    crate::sessions::list::Key::External(pid) => json!({ "external": pid }),
                };
                json!({
                    "key": key,
                    "name": entry.name,
                    "space": entry.space,
                    "state": entry.state.to_string(),
                    "recap": entry.recap,
                    "pinned": entry.pinned,
                    "archived": entry.archived,
                })
            })
            .collect();
        let list = self
            .watch
            .view
            .buffer
            .filter(|_| self.sessions_list_shown())
            .and_then(|id| self.editor.buffer(id))
            .map(|buffer| buffer.text().to_string());
        json!({
            "sessions": sessions,
            "side": self.editor.side_window().map(|id| id.get()),
            "list": list,
            "space": self
                .watch
                .spaces
                .current()
                .and_then(|at| self.watch.spaces.get(at))
                .map(|space| space.name.clone()),
            "asked": self.watch.asked,
            "focused": self.terminals.focused,
        })
    }
}
