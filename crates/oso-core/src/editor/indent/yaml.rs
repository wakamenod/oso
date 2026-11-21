//! YAML's `TAB` (ADR 0164): each cursor's line cycles through the columns it could take, as
//! Emacs's yaml-mode cycles: the deepest the line before allows, then each level out to 0.
//!
//! Must not depend on: `command` or `keymap`; the commands call this.
//!
//! The cycle keeps no state between keys: a line at one of its stops goes to the next, and one
//! at none goes to the first, so that a keyboard macro replays the same.

use super::{has_indentation, leading};
use crate::buffer::{EditError, Replacement};
use crate::editor::Editor;
use crate::indent::{self, Budget};

impl Editor {
    /// `TAB` in YAML: each cursor's line goes to the next of its stops ([`indent::yaml_stops`]),
    /// and a line at none of them to the first. One edit; a cursor inside the indentation goes
    /// to its end.
    pub(super) fn yaml_tab(&mut self) -> Result<(), EditError> {
        let (window, buffer) = self.current();
        if let Some(reason) = buffer.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
        let id = window.buffer();
        let rope = buffer.text();
        let settings = buffer.indent_settings();
        let mut rows: Vec<usize> = window
            .selections
            .iter()
            .map(|selection| rope.offset_to_point(selection.cursor).row)
            .collect();
        rows.dedup();
        // One budget for the command, as the rules of the other modes share one.
        let mut budget = Budget::new();
        let mut texts = Vec::new();
        for &row in &rows {
            let stops = indent::yaml_stops(rope, settings, row, &mut budget);
            let start = rope.line_start(row);
            let (bytes, now) = leading(rope, start, settings.tab_width());
            let column = match stops.iter().position(|&stop| stop == now) {
                Some(at) => stops[(at + 1) % stops.len()],
                None => stops.first().copied().unwrap_or(0),
            };
            if !has_indentation(rope, start..start + bytes, now, column, settings) {
                texts.push((start..start + bytes, settings.indentation(column)));
            }
        }
        if !texts.is_empty() {
            let edits: Vec<_> = texts
                .iter()
                .map(|(range, text)| Replacement {
                    range: range.clone(),
                    text,
                })
                .collect();
            self.apply_edits(id, &edits)?;
        }
        self.cursors_past_indentation(&rows);
        Ok(())
    }
}
