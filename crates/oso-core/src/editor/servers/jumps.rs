//! Jumps a language server's answers lead to, and back (ADR 0138, Go to definition and back,
//! step 8): where an ask about the selected buffer is, a jump to a place in an open buffer or
//! in a file to visit, and `M-,` and `C-M-,` through the selected window's history of jumps.
//!
//! Must not depend on: the host's servers, threads or I/O; the host asks the server and hands
//! over where its answer leads.
//!
//! A jump keeps the place it leaves in the window's history and on the mark ring there, as
//! avy's jumps do, and ends with one cursor at the target, with the place before on the
//! target's mark ring, as a visit at a place does (ADR 0114).

use std::path::PathBuf;

use crate::buffer::BufferId;
use crate::selection::OffsetEdit;
use crate::sum_tree::Bias;

use super::super::Editor;
use super::super::visits::VisitColumn;

/// Where an ask about a buffer is: its primary cursor in the selected window, in the text of
/// `revision`, and whether the agent's command asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AskAt {
    pub buffer: BufferId,
    pub revision: u64,
    pub offset: usize,
    pub agent: bool,
}

impl Editor {
    /// Where an ask about the selected buffer is now.
    pub fn ask_at(&self) -> AskAt {
        let (window, buffer) = self.current();
        AskAt {
            buffer: window.buffer(),
            revision: buffer.revision(),
            offset: window.selections.primary().cursor,
            agent: self.runs_for_agent(),
        }
    }

    /// Jumps the selected window to `offset` in `buffer`, an open buffer, clipped to its text
    /// and to a display cluster's start. Does nothing for a buffer that does not exist, or a
    /// jump to where the primary cursor is.
    pub fn xref_to_place(&mut self, buffer: BufferId, offset: usize) {
        let Some(target) = self.place_in(buffer, offset) else {
            return;
        };
        if self.here() == (buffer, target) {
            return;
        }
        self.keep_xref_start();
        self.go_to(buffer, target);
    }

    /// Jumps the selected window to `line`, from 1, and `column` in `path`, visited as
    /// [`Editor::request_visit_at`] visits it, reading at most `limit` bytes.
    pub fn xref_visit(&mut self, path: PathBuf, limit: u64, line: usize, column: VisitColumn) {
        self.keep_xref_start();
        self.request_visit_at_column(path, limit, line, column);
    }

    /// `M-,`: back to the nearest place in the selected window's history. `Err` says why not.
    pub fn xref_back(&mut self) -> Result<(), &'static str> {
        self.walk_jumps(false)
    }

    /// `C-M-,`: forward again to the place the last `M-,` left. `Err` says why not.
    pub fn xref_forward(&mut self) -> Result<(), &'static str> {
        self.walk_jumps(true)
    }

    /// Goes to the nearest place `forward` or back, the history changed only once it can. The
    /// agent goes to no buffer it may not be shown (ADR 0094).
    fn walk_jumps(&mut self, forward: bool) -> Result<(), &'static str> {
        let none = if forward {
            "No later place"
        } else {
            "No earlier place"
        };
        let (buffer, offset) = self.windows[self.selected]
            .jumps()
            .nearest(forward)
            .ok_or(none)?;
        let shown = self.buffer(buffer).ok_or(none)?;
        if self.runs_for_agent()
            && (shown.is_secret()
                || shown.withheld_from_agents()
                || shown.agent_pending()
                || shown.loading().is_some())
        {
            return Err("That place's buffer is not for the agent");
        }
        let target = self.place_in(buffer, offset).ok_or(none)?;
        let here = self.here();
        let jumps = self.windows[self.selected].jumps_mut();
        if forward {
            jumps.forward(here);
        } else {
            jumps.back(here);
        }
        self.go_to(buffer, target);
        Ok(())
    }

    /// Moves the places in `buffer` of every window's history through an edit's `steps`.
    pub(in crate::editor) fn follow_jump_history(
        &mut self,
        buffer: BufferId,
        steps: &[Vec<OffsetEdit>],
    ) {
        for window in &mut self.windows {
            window.jumps_mut().follow(buffer, steps);
        }
    }

    /// Moves the places in `buffer` through what a sliced undo's step did since `before`, as
    /// the batch the buffer keeps says; a buffer that keeps none leaves them (ADR 0119).
    pub(in crate::editor) fn jumps_cross(&mut self, buffer: BufferId, before: u64) {
        let Some(splices) = self
            .buffer(buffer)
            .and_then(|shown| shown.edits_since(before))
        else {
            return;
        };
        let mut edits: Vec<OffsetEdit> = splices
            .iter()
            .map(|splice| OffsetEdit {
                start: splice.start,
                old_len: splice.old_len,
                new_len: splice.new_len,
            })
            .collect();
        edits.sort_unstable_by_key(|edit| edit.start);
        self.follow_jump_history(buffer, &[edits]);
    }

    /// Moves the places in `buffer` of every window's history through `map`, as a reload
    /// moves the cursors.
    pub(in crate::editor) fn map_jump_history(
        &mut self,
        buffer: BufferId,
        mut map: impl FnMut(usize) -> usize,
    ) {
        for window in &mut self.windows {
            window.jumps_mut().map(buffer, &mut map);
        }
    }

    fn here(&self) -> (BufferId, usize) {
        let window = &self.windows[self.selected];
        (window.buffer(), window.selections.primary().cursor)
    }

    /// Keeps the selected window's place in its history, and on its mark ring unless a region
    /// shows, as avy's jumps do (ADR 0116): for a jump the host makes in its own way, as the
    /// agent channel's open.
    pub fn keep_xref_start(&mut self) {
        let here = self.here();
        let window = &mut self.windows[self.selected];
        window.jumps_mut().push(here);
        if !window.selections.mark_active().is_active() {
            window.selections.push_primary_mark(here.1);
        }
    }

    /// `offset` in `buffer` as far as its text goes, on a display cluster's start.
    fn place_in(&self, buffer: BufferId, offset: usize) -> Option<usize> {
        let shown = self.buffer(buffer)?;
        let clipped = shown.text().clip_offset(offset, Bias::Left);
        Some(crate::motion::cluster_start(&shown.display(), clipped))
    }

    /// Shows `buffer` in the selected window and lands its primary cursor, alone, at `target`,
    /// a cluster's start, as a visit at a place lands.
    fn go_to(&mut self, buffer: BufferId, target: usize) {
        let index = self.selected;
        if self.windows[index].buffer() != buffer {
            self.show(buffer);
            if self.windows[index].buffer() != buffer {
                return;
            }
        }
        self.land_primary(index, target);
    }
}

#[cfg(test)]
mod tests;
