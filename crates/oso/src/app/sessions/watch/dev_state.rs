//! What the dev socket's state says of agent sessions (ADR 0121 step 3), for end-to-end flows:
//! each session's state and recap, whether `*Spaces*` shows and its lines, the marks, how
//! many notices asked for attention, and the issue spaces (ADR 0136). The dev socket trusts the same user (ADR 0044); the
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
        // Each space as end-to-end flows check it (ADR 0136): its id, name, repositories,
        // worktrees and whether it is archived.
        let spaces: Vec<_> = self
            .watch
            .spaces
            .iter()
            .map(|space| {
                let stored = &space.stored;
                json!({
                    "id": stored.id.as_str(),
                    "name": stored.name,
                    "members": stored.members.iter().map(|m| m.root.display().to_string()).collect::<Vec<_>>(),
                    "worktrees": stored.worktrees.iter().map(|w| w.display().to_string()).collect::<Vec<_>>(),
                    "archived": stored.archived,
                    "display": stored.display,
                    "description": stored.description,
                    "orchestrator": stored.orchestrator.as_ref().map(|id| id.as_str().to_owned()),
                    "directory": stored.directory.as_ref().map(|d| d.display().to_string()),
                })
            })
            .collect();
        json!({
            "sessions": sessions,
            "spaces": spaces,
            "side": self.editor.side_window().map(|id| id.get()),
            "list": list,
            // The lines drawn inverse on the blink's lit beat (ADR 0140, as amended).
            "lit": self.watch.pane_bands().lit.iter().map(|(line, _)| *line).collect::<Vec<_>>(),
            "space": self
                .watch
                .spaces
                .current()
                .and_then(|at| self.watch.spaces.get(at))
                .map(|space| space.name().to_owned()),
            "asked": self.watch.asked,
            "focused": self.terminals.focused,
        })
    }
}
