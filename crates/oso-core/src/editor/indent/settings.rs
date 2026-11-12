//! Each buffer's indentation setting (ADR 0115): its mode's, as the configuration gives it,
//! under what the EditorConfig files read for its file give, with where each property came
//! from; and the files whose EditorConfig the host should read again after `C-x C-w`.
//!
//! Must not depend on: `command` or `keymap`; the host and the commands call this.

use super::super::{Editor, ProjectRequest};
use crate::buffer::{Buffer, BufferId, Mode};
use crate::editorconfig::{Applied, IndentSize};
use crate::indent::{self, Origin, Origins, Settings};

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
        let shown = buffer.tab_width();
        let (mut settings, mut origins) = match buffer.editorconfig() {
            Some(applied) => indent::with_editorconfig(configured, given, &applied.properties),
            None => (configured, given),
        };
        if settings.tab_width() != shown {
            origins.tab_width = Origin::Visited;
            // A level of `tab` is a tab as it is shown.
            let tab_level = buffer
                .editorconfig()
                .and_then(|applied| applied.properties.indent_size())
                .is_some_and(|(size, _)| size == IndentSize::Tab);
            let size = if tab_level {
                shown.get()
            } else {
                u32::try_from(settings.indent_size()).unwrap_or(4)
            };
            settings = Settings::new(size, settings.style(), shown.get()).unwrap_or(settings);
        }
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_indent_settings(settings);
            buffer.set_indent_origins(origins);
        }
    }

    /// Asks the host to read the EditorConfig of `id`'s file again, as `C-x C-w` named it.
    pub(crate) fn request_editorconfig(&mut self, id: BufferId) {
        let Some(location) = self
            .buffer(id)
            .filter(|buffer| !buffer.is_internal())
            .and_then(Buffer::baseline)
            .map(|baseline| baseline.location.clone())
        else {
            return;
        };
        self.editorconfig_requests
            .retain(|asked| asked.buffer != id);
        self.editorconfig_requests.push(ProjectRequest {
            buffer: id,
            location,
        });
    }

    /// The files whose EditorConfig the host should read again, the oldest first. What it
    /// reads applies with [`Editor::set_editorconfig`], if the buffer's file is still there.
    pub fn take_editorconfig_requests(&mut self) -> Vec<ProjectRequest> {
        std::mem::take(&mut self.editorconfig_requests)
    }
}

#[cfg(test)]
mod tests;
