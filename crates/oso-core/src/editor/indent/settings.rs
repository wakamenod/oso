//! Each buffer's indentation setting (ADR 0115): its mode's, as the configuration gives it,
//! under what the EditorConfig files read for its file give, with where each property came
//! from; and the files whose EditorConfig the host should read again after `C-x C-w`.
//!
//! Must not depend on: `command` or `keymap`; the host and the commands call this.

use super::super::{Editor, ProjectRequest};
use crate::buffer::{Buffer, BufferId, Mode};
use crate::editorconfig::{Applied, Properties};
use crate::indent::{self, Origin, Origins, Settings};

/// Each mode's indentation, as the configuration gives it, and where each property came from.
pub(in crate::editor) type ModeIndents = [(Mode, Settings, Origins); 4];

/// Each mode's compiled-in indentation, before the configuration gives one.
pub(in crate::editor) fn compiled_mode_indents() -> ModeIndents {
    [Mode::Rust, Mode::Markdown, Mode::Toml, Mode::Fundamental]
        .map(|mode| (mode, Settings::default(), Origins::default()))
}

impl Editor {
    /// Sets each mode's indentation, as the configuration gives it, with where each property
    /// came from, and gives every buffer of the person's its mode's indent size and style under
    /// what EditorConfig gives it. A buffer's tab width stays the one it was visited with,
    /// which its display map is built with.
    pub fn set_mode_indents(&mut self, indents: [(Mode, Settings, Origins); 4]) {
        self.mode_indents = indents;
        let ids: Vec<BufferId> = self.buffers().map(|(id, _)| id).collect();
        for id in ids {
            self.give_mode_indent(id);
        }
    }

    /// The indentation of `mode`: the configuration's, or four spaces with tabs eight wide.
    pub fn mode_indent(&self, mode: Mode) -> Settings {
        self.configured_indent(mode).0
    }

    /// The indentation of `mode`, with where each property came from.
    fn configured_indent(&self, mode: Mode) -> (Settings, Origins) {
        self.mode_indents
            .iter()
            .find(|(known, ..)| *known == mode)
            .map_or_else(Default::default, |&(_, settings, origins)| {
                (settings, origins)
            })
    }

    /// Sets what the EditorConfig files read for `id`'s file give it, and gives it its mode's
    /// indentation under that (ADR 0115).
    pub fn set_editorconfig(&mut self, id: BufferId, applied: Option<Applied>) {
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_editorconfig(applied);
        }
        self.give_mode_indent(id);
    }

    /// Gives the buffer `id` its mode's indent size and style, under what EditorConfig gives
    /// it, unless it is internal: as a visit, a mode command, a new name that changes its mode
    /// and a configuration's change do. Its tab width stays the one it is shown with; one
    /// that differs applies at the next visit.
    pub(crate) fn give_mode_indent(&mut self, id: BufferId) {
        let Some(buffer) = self.buffer(id).filter(|b| !b.is_internal()) else {
            return;
        };
        let (configured, given) = self.configured_indent(buffer.mode());
        let none = Properties::default();
        let properties = buffer.editorconfig().map_or(&none, Applied::properties);
        let (settings, mut origins) =
            indent::with_editorconfig(configured, given, properties, buffer.tab_width());
        // A buffer that visits no file is shown with the default.
        if origins.tab_width == Origin::Visited && buffer.path().is_none() {
            origins.tab_width = Origin::Default;
        }
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_indent_settings(settings);
            buffer.set_indent_origins(origins);
        }
    }

    /// Sets the mode of the buffer `id`, and gives it the mode's indentation under what
    /// EditorConfig gave it. Returns false if its mode cannot change, as an internal buffer's.
    pub fn set_buffer_mode(&mut self, id: BufferId, mode: Mode) -> bool {
        let set = self
            .buffer_mut(id)
            .is_some_and(|buffer| buffer.set_mode(mode));
        if set {
            self.give_mode_indent(id);
        }
        set
    }

    /// Asks the host to read the EditorConfig of `id`'s file again, as `C-x C-w` named it, or
    /// as a buffer carried over a dev restart needs, once where its path leads is known.
    pub fn request_editorconfig(&mut self, id: BufferId) {
        let internal = self.buffer(id).is_none_or(Buffer::is_internal);
        if let Some(request) = self.file_request(id).filter(|_| !internal) {
            super::super::projects::ask(&mut self.editorconfig_requests, request);
        }
    }

    /// The files whose EditorConfig the host should read again, the oldest first. What it
    /// reads comes back through [`Editor::editorconfig_read`].
    pub fn take_editorconfig_requests(&mut self) -> Vec<ProjectRequest> {
        std::mem::take(&mut self.editorconfig_requests)
    }

    /// The host read `applied` for `request`'s file. It applies only if the buffer's path still
    /// leads where it did when asked, as a project's answer does. Returns whether it applied.
    pub fn editorconfig_read(
        &mut self,
        request: &ProjectRequest,
        applied: Option<Applied>,
    ) -> bool {
        let current = self
            .buffer(request.buffer)
            .and_then(Buffer::baseline)
            .is_some_and(|baseline| baseline.location == request.location);
        if current {
            self.set_editorconfig(request.buffer, applied);
        }
        current
    }
}

#[cfg(test)]
mod tests;
