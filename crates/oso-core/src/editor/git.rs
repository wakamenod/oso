//! Git status buffers (ADR 0127): each one's record, the text the host builds for it off the UI
//! thread, and what commands ask of the host for them.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads, files or processes; the
//! host runs git on its workers and hands each status text in whole.
//!
//! - A status buffer is an internal, read-only buffer in `GitStatus` mode that does not wrap,
//!   one for each project, `*git: NAME*`. As magit's are, and as a directory's listing is, it is
//!   listed by `C-x b` and killed by `C-x k`.
//! - Its text comes whole, with its display map, and goes in at O(1), as a file's load does.
//! - What a line names (a section, a file, a hunk) is the host's: the editor keeps only the
//!   outline, the line and depth of each heading, so that motion between sections works in the
//!   editor, and a command sends the lines it means.

use super::Editor;
use crate::buffer::{Buffer, BufferId, Mode};
use crate::load::LoadSnapshot;
use crate::project::ProjectId;
use crate::selection::SelectionSet;
use crate::syntax::Highlights;
use crate::window::WindowId;

/// The lines one selection covers in a status buffer: from `first` to `last`, both included,
/// and whether it is a region, which means its lines, or a cursor, which means what it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitLines {
    pub first: usize,
    pub last: usize,
    pub region: bool,
}

/// A heading of a status buffer's outline: a section's, a file's or a hunk's, at `line`, with
/// its depth, 0 for a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitHead {
    pub line: usize,
    pub depth: u8,
}

/// What a command asks of the host for git (ADR 0127).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitRequest {
    /// Show the status buffer of `project` in the selected window, read again: `C-x g`.
    Status { project: ProjectId },
    /// Read the status again: `g`.
    Refresh { buffer: BufferId },
    /// Show or hide what the heading at or above `line` holds: a file's hunks, or a section's
    /// rows (`TAB`).
    Toggle { buffer: BufferId, line: usize },
    /// Stage what each of `lines` is on (`s`).
    Stage {
        buffer: BufferId,
        lines: Vec<GitLines>,
    },
    /// Unstage what each of `lines` is on (`u`).
    Unstage {
        buffer: BufferId,
        lines: Vec<GitLines>,
    },
    /// Stage every unstaged change shown (`S`).
    StageAll { buffer: BufferId },
    /// Unstage every staged change shown (`U`).
    UnstageAll { buffer: BufferId },
    /// Visit the file `line` is about, at the line of the file it shows, if any (`RET`).
    Visit {
        buffer: BufferId,
        line: usize,
        column: usize,
    },
}

/// A status buffer's record.
#[derive(Debug)]
struct Status {
    project: ProjectId,
    outline: Vec<GitHead>,
}

/// The status buffers and what commands asked for them.
#[derive(Debug, Default)]
pub(super) struct Statuses {
    records: Vec<(BufferId, Status)>,
    requests: Vec<GitRequest>,
}

/// What [`Editor::set_git_text`] puts in beside the text.
#[derive(Debug)]
pub struct GitShown {
    /// The highlights, built with the text, covering it whole.
    pub highlights: Highlights,
    pub outline: Vec<GitHead>,
    /// The line a window's cursor goes to unless `lines` says.
    pub line: usize,
    /// The line each window's cursor goes to.
    pub lines: Vec<(WindowId, usize)>,
}

impl Editor {
    /// Asks the host for `request`.
    pub fn request_git(&mut self, request: GitRequest) {
        self.git.requests.push(request);
    }

    /// What commands asked of the host for git, oldest first.
    pub fn take_git_requests(&mut self) -> Vec<GitRequest> {
        std::mem::take(&mut self.git.requests)
    }

    /// Adds an empty status buffer for `project`, named `name`, shown in no window yet, which
    /// does not wrap.
    pub fn add_git_status(&mut self, project: ProjectId, name: impl Into<String>) -> BufferId {
        let id = self.add_internal_buffer(name, Mode::GitStatus);
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_wraps(false);
        }
        self.git.records.push((
            id,
            Status {
                project,
                outline: Vec::new(),
            },
        ));
        id
    }

    /// The project of the status buffer `buffer`, if it is one and is still there.
    pub fn git_status_of(&self, buffer: BufferId) -> Option<ProjectId> {
        self.buffer(buffer)?;
        self.git_record(buffer).map(|status| status.project)
    }

    /// The status buffer of `project`, if one is open.
    pub fn git_status_buffer(&self, project: ProjectId) -> Option<BufferId> {
        self.git_statuses()
            .find(|(_, of)| *of == project)
            .map(|(id, _)| id)
    }

    /// Every status buffer still there, with its project.
    pub fn git_statuses(&self) -> impl Iterator<Item = (BufferId, ProjectId)> {
        (self.git.records.iter())
            .filter(|(id, _)| self.buffer(*id).is_some())
            .map(|(id, status)| (*id, status.project))
    }

    /// The headings of the status buffer `buffer`, in order: empty for any other buffer.
    pub fn git_outline(&self, buffer: BufferId) -> &[GitHead] {
        self.git_record(buffer)
            .map_or(&[], |status| status.outline.as_slice())
    }

    /// Each window's id and the buffer line its cursor is on, for the windows that show the
    /// status buffer `id`, so that the host can tell what each is on before it puts in a new
    /// text.
    pub fn git_points(&self, id: BufferId) -> Vec<(WindowId, usize)> {
        let Some(buffer) = self.buffer(id) else {
            return Vec::new();
        };
        let text = buffer.text();
        self.windows
            .iter()
            .filter(|window| window.buffer() == id)
            .map(|window| {
                let cursor = window.selections.primary().cursor;
                (window.id(), text.offset_to_point(cursor).row)
            })
            .collect()
    }

    /// Puts in the whole text of the status buffer `id`, built off the UI thread, with its
    /// outline and highlights. O(1) for the text: each window that shows it has its cursor at
    /// the start of the line `lines` gives for it, else of `line`, and a window that will come
    /// back to it at `line` too. No undo. The host keeps the highlights given back, which the
    /// new ones replaced or which did not fit, to free them off the UI thread.
    pub fn set_git_text(
        &mut self,
        id: BufferId,
        text: LoadSnapshot,
        shown: GitShown,
    ) -> Option<Highlights> {
        let GitShown {
            highlights,
            outline,
            line,
            lines,
        } = shown;
        if self.git_record(id).is_none() {
            return Some(highlights);
        }
        let Some(buffer) = self.buffer_mut(id).filter(|buffer| is_git_status(buffer)) else {
            return Some(highlights);
        };
        buffer.apply_load(text);
        let old = buffer.take_generated_highlights();
        let refused = if buffer.text().len() == highlights.len() {
            buffer.set_generated_highlights(highlights).or(old)
        } else {
            Some(highlights)
        };
        let Some(buffer) = self.buffer(id) else {
            return refused;
        };
        let text = buffer.text();
        let start_of = |line: usize| text.line_start(line.min(text.max_point().row));
        let default = start_of(line);
        let placed: Vec<(WindowId, usize)> = lines
            .iter()
            .map(|(window, line)| (*window, start_of(*line)))
            .collect();
        for window in &mut self.windows {
            if let Some(place) = window.places_mut().get_mut(id) {
                place.selections = SelectionSet::new(default);
            }
            if window.buffer() != id {
                continue;
            }
            let at = placed
                .iter()
                .find(|(shown, _)| *shown == window.id())
                .map_or(default, |(_, at)| *at);
            window.selections = SelectionSet::new(at);
        }
        if let Some((_, status)) = self.git.records.iter_mut().find(|(of, _)| *of == id) {
            status.outline = outline;
        }
        self.loaded(id);
        if self.window().buffer() == id {
            self.follow_cursor();
        }
        refused
    }

    /// Drops the record of the status buffer `id`, as its buffer goes.
    pub(super) fn forget_git_status(&mut self, id: BufferId) {
        self.git.records.retain(|(of, _)| *of != id);
    }

    fn git_record(&self, id: BufferId) -> Option<&Status> {
        (self.git.records.iter())
            .find(|(of, _)| *of == id)
            .map(|(_, status)| status)
    }
}

/// Whether `buffer` is a status buffer.
pub(super) fn is_git_status(buffer: &Buffer) -> bool {
    buffer.is_internal() && buffer.mode() == Mode::GitStatus
}

#[cfg(test)]
mod tests;
