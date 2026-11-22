//! A native session's tool calls among its rows (ADR 0165): adding a call, to a row of its own
//! or to the last row it may share, putting the whole call in its place, changing a call
//! found by its id, finding one that waits for the person's permission, and saying how the
//! person answered it (step 2).
//!
//! Must not depend on: threads, files or the UI; methods of [`Rows`] over its own state.

use std::path::Path;

use oso_core::buffer::Buffer;

use super::{
    GROUPED, Kind, MAX_GROUP, Open, Row, RowChange, Rows, Task, Tool, ToolInput, ToolState,
};

impl Rows {
    /// Adds a tool call: to the last row when it and every call there are calls of one tool
    /// that [`GROUPED`] names and may share a row, else as a row of its own. Says what changed,
    /// the row and the call's place in it.
    pub(super) fn add_call(&mut self, call: Tool, depth: u8) -> (RowChange, usize, usize) {
        let id = call.id.clone();
        self.turn.push(id.clone());
        let last = self.rows.len().wrapping_sub(1);
        let shares = self.may_share(&call)
            && matches!(self.rows.last(), Some(Row { kind: Kind::Tools(tools), .. })
                if tools.iter().all(|tool| !tool.apart && self.may_share(tool)));
        if let Some(Row {
            kind: Kind::Tools(tools),
            depth: at,
            ..
        }) = self.rows.last_mut()
            && *at == depth
            && shares
            && tools.len() < MAX_GROUP
            && tools.iter().all(|tool| tool.name == call.name)
        {
            tools.push(call);
            let slot = tools.len() - 1;
            self.calls.insert(id, (last, slot));
            return (RowChange::Changed(last), last, slot);
        }
        let change = self.push(Kind::Tools(vec![call]), depth, true);
        let row = self.rows.len() - 1;
        self.calls.insert(id, (row, 0));
        (change, row, 0)
    }

    /// Whether `call` may share a row: a read or a search inside the session's directory, of
    /// nothing that may hold secrets.
    pub(super) fn may_share(&self, call: &Tool) -> bool {
        if !GROUPED.contains(&call.name.as_str()) || call.input.cut {
            return false;
        }
        // A Glob's pattern names paths; a Grep's matches text.
        let pattern_inside = call.name == "Grep"
            || call
                .input
                .text
                .as_deref()
                .is_none_or(|text| !text.starts_with(['/', '~', '\\']) && !text.contains(".."));
        let path_inside = call
            .input
            .path
            .as_deref()
            .is_none_or(|path| self.inside(Path::new(path)));
        pattern_inside && path_inside
    }

    /// Whether `path` is inside the session's directory and names nothing that may hold
    /// secrets (ADR 0044).
    pub(super) fn inside(&self, path: &Path) -> bool {
        let path = self.directory.join(path);
        path.starts_with(&self.directory)
            && !path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            && !Buffer::judge_name(&path).is_withheld()
    }

    /// Puts a whole tool call in its open place, or adds it. A call that joined a row while
    /// its input streamed and may not share it leaves for a row of its own when it is the
    /// last call of the last row, and is marked [`Tool::apart`] when it is not.
    pub(super) fn fill_call(
        &mut self,
        open: Option<Open>,
        mut call: Tool,
        depth: u8,
    ) -> Vec<RowChange> {
        let Some(open) = open else {
            return vec![self.add_call(call, depth).0];
        };
        let shares = self.may_share(&call);
        let last_row = open.row + 1 == self.rows.len();
        let Kind::Tools(tools) = &mut self.rows[open.row].kind else {
            return Vec::new();
        };
        let count = tools.len();
        let Some(place) = tools.get_mut(open.slot) else {
            return Vec::new();
        };
        call.folded = place.folded;
        if count == 1 || shares {
            *place = call;
            return vec![RowChange::Changed(open.row)];
        }
        if last_row && open.slot + 1 == count {
            tools.pop();
            let id = call.id.clone();
            let change = self.push(Kind::Tools(vec![call]), depth, true);
            self.calls.insert(id, (self.rows.len() - 1, 0));
            return vec![RowChange::Changed(open.row), change];
        }
        call.apart = true;
        *place = call;
        vec![RowChange::Changed(open.row)]
    }

    pub(super) fn task(&mut self, task: Task) -> Option<RowChange> {
        let id = task.tool_use_id?;
        let description = task.description?;
        self.update_call(&id, |tool| tool.progress = Some(description))
    }

    /// Changes the call `id`, if there is one, and says its row changed.
    pub(super) fn update_call(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut Tool),
    ) -> Option<RowChange> {
        let &(row, slot) = self.calls.get(id)?;
        let Kind::Tools(tools) = &mut self.rows.get_mut(row)?.kind else {
            return None;
        };
        change(tools.get_mut(slot)?);
        Some(RowChange::Changed(row))
    }

    /// The person answered call `id` on its card (step 2e-2): a denial says so on its row, with
    /// the note the CLI was told; an allowed call's row goes on to say how it ran.
    pub fn answered(&mut self, id: &str, allowed: bool, note: Option<&str>) -> Option<RowChange> {
        if allowed {
            return None;
        }
        let note = note.unwrap_or("by the person").to_owned();
        self.update_call(id, |tool| tool.state = ToolState::Denied(note))
    }

    /// The call `id` of a whole message, which runs and has no result yet: what the person is
    /// asked about when the CLI asks permission for it (ADR 0165, step 2). `None` while its
    /// message streams, once it has a result, or if there is none.
    pub fn waiting_call(&self, id: &str) -> Option<&Tool> {
        let &(row, slot) = self.calls.get(id)?;
        let Kind::Tools(tools) = &self.rows.get(row)?.kind else {
            return None;
        };
        let tool = tools.get(slot)?;
        (tool.state == ToolState::Running && tool.result.is_none()).then_some(tool)
    }
}

/// A call not yet answered, folded.
pub(super) fn new_call(id: String, name: String, input: ToolInput, state: ToolState) -> Tool {
    Tool {
        id,
        name,
        input,
        streamed: 0,
        state,
        result: None,
        progress: None,
        folded: true,
        apart: false,
    }
}
