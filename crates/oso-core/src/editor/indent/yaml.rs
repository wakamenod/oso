//! YAML's `TAB` (ADR 0164): each cursor's line cycles through the columns it could take, as
//! Emacs's yaml-mode cycles: the deepest the code line before allows, then each level out to 0.
//!
//! Must not depend on: `command` or `keymap`; the commands call this.
//!
//! The cycle keeps no state between keys: a line at one of its stops goes to the next, and one
//! at none goes to the first, so that a keyboard macro replays the same. A line whose stops
//! cannot be told, past the reach or the budget, keeps its indentation: in YAML a line's
//! indentation is what it means.

use super::leading;
use crate::buffer::EditError;
use crate::editor::Editor;
use crate::indent::{self, Budget};

impl Editor {
    /// `TAB` in YAML: each cursor's line goes to the next of its stops ([`indent::yaml_stops`]),
    /// and a line at none of them to the first. One edit; a cursor inside the indentation of a
    /// line cycled goes to its end.
    pub(super) fn yaml_tab(&mut self) -> Result<(), EditError> {
        let (window, buffer) = self.current();
        if let Some(reason) = buffer.read_only() {
            return Err(EditError::ReadOnly(reason));
        }
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
        let mut columns = Vec::with_capacity(rows.len());
        for row in rows {
            let stops = indent::yaml_stops(rope, settings, row, &mut budget);
            let now = leading(rope, rope.line_start(row), settings.tab_width()).1;
            let column = match stops.iter().position(|&stop| stop == now) {
                Some(at) => stops[(at + 1) % stops.len()],
                None => match stops.first() {
                    Some(&first) => first,
                    None => continue,
                },
            };
            columns.push((row, column));
        }
        self.set_columns(&columns)?;
        let cycled: Vec<usize> = columns.into_iter().map(|(row, _)| row).collect();
        self.cursors_past_indentation(&cycled);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
