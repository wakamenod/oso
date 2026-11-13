//! What the dev socket's state says of each terminal (ADR 0120), for end-to-end flows. The dev
//! socket trusts the same user (ADR 0044); the agent channel reads none of this.
//!
//! Must not depend on: how a frame is drawn.

use oso_core::buffer::BufferId;

use super::App;

impl App {
    /// Each terminal as the dev socket's state gives it: its buffer, whether a program runs,
    /// how it reads keys, its size, scroll, modes, cursor and visible rows' text. While its
    /// program reads a password the rows are null, as dev tooling withholds the text of any
    /// buffer that may hold secrets (ADR 0044, ADR 0120 Q17).
    pub(in crate::app) fn terminals_state(&self) -> serde_json::Value {
        let mut terminals: Vec<(BufferId, serde_json::Value)> = self
            .terminals
            .map
            .iter()
            .map(|(id, hosted)| {
                let grid = &hosted.grid;
                let size = grid.size();
                let secret = self.editor.buffer(id).is_some_and(super::reads_a_password);
                let rows: Option<Vec<String>> = (!secret).then(|| {
                    (0..size.map_or(0, |size| usize::from(size.rows)))
                        .filter_map(|row| grid.row_text(row))
                        .collect()
                });
                let (row, column) = grid.cursor_cell();
                let modes = grid.modes();
                let input = match self.dispatcher.terminal_input(id) {
                    crate::terminal::route::Input::SemiChar => "semi-char",
                    crate::terminal::route::Input::Char => "char",
                };
                let value = serde_json::json!({
                    "id": id.get(),
                    "name": self.editor.buffer(id).map(|buffer| buffer.name().to_owned()),
                    "started": hosted.terminal.is_some() || hosted.ended.is_some(),
                    "running": hosted.running() && hosted.terminal.is_some(),
                    "ended": hosted.ended,
                    "title": hosted.title,
                    "echo_off": secret,
                    "input": input,
                    "size": size.map(|size| serde_json::json!({
                        "rows": size.rows,
                        "columns": size.columns,
                    })),
                    "owner": hosted.owner.map(oso_core::window::WindowId::get),
                    "scrolled": grid.scrolled(),
                    "history": grid.history(),
                    "bracketed_paste": modes.bracketed_paste,
                    "cursor": { "row": row, "column": column },
                    "pending_paste": hosted.pending_paste.is_some(),
                    "rows": rows,
                });
                (id, value)
            })
            .collect();
        terminals.sort_by_key(|(id, _)| *id);
        serde_json::Value::Array(terminals.into_iter().map(|(_, value)| value).collect())
    }
}
