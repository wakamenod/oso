//! A native session's buffer's keys carried out (ADR 0165, ADR 0130's design): `TAB` folds the
//! row at point, or the call of an open row of several that point is on, `RET` shows a call's
//! whole result in `*result: NAME*`, and `n`/`p`, `]`/`[` move to the start of a prompt's row
//! or a tool row. The composer's keys and the queue's (step 2) go to `composer` first: from
//! the composer, for the session it is docked under; from the transcript, at point's line.
//!
//! Must not depend on: key presses; the command gave the line point is on, and `Placed` says
//! which row it is in.
//!
//! - `*result: NAME*` is internal, read-only and withheld from agents (B1, as
//!   `*Session explain*`): a result may hold what the session read. One per session, which
//!   `RET` fills again, at the cost of the result's size (at most `wire`'s 1 MiB), on the
//!   person's key; the call it shows already is shown as it is.
//! - Where each call of an open row of several is, `draw` keeps as it draws the row, so a key
//!   finds the call point is on without drawing the row again.

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::NativeAction;

use super::{Native, draw};
use crate::app::App;
use crate::app::sessions::show_withheld;
use crate::sessions::native::point::{self, Point, Wanted};
use crate::sessions::native::rows::Kind;
use crate::sessions::worker::SessionKey;

impl App {
    /// Carries out `action` at line `line` of native session buffer `buffer`.
    pub(in crate::app) fn native_key(
        &mut self,
        buffer: BufferId,
        line: usize,
        action: NativeAction,
    ) {
        // From the composer, the session it is docked under (step 2).
        if let Some(key) = self.composer_session(buffer) {
            self.composer_key(key, None, action);
            return;
        }
        let key = self.hosting.table().by_buffer(buffer).map(|s| s.key);
        let Some(key) = key.filter(|key| self.hosting.natives.has(*key)) else {
            return self.editor.message("This buffer holds no native session");
        };
        let Some(offset) = self
            .editor
            .buffer(buffer)
            .map(|b| b.text().line_start(line))
        else {
            return;
        };
        if self.composer_key(key, Some(offset), action) {
            return;
        }
        match action {
            NativeAction::Toggle => self.native_toggle(key, offset),
            NativeAction::Open => self.native_result(key, offset),
            NativeAction::Prompt(count) => self.native_move(key, offset, count, Wanted::Prompt),
            NativeAction::Tool(count) => self.native_move(key, offset, count, Wanted::Tools),
            // The composer's and the queue's, carried out above.
            _ => {}
        }
    }

    /// Folds or unfolds the row, or the call, at `offset`, and puts point at its start.
    fn native_toggle(&mut self, key: SessionKey, offset: usize) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let Some((row, slot, start)) = at(native, offset) else {
            return self.editor.message("Nothing folds here");
        };
        let Some(change) = native.rows.toggle(row, slot) else {
            return self.editor.message("This row does not fold");
        };
        let buffer = native.buffer;
        draw(&mut self.editor, native, [change]);
        self.native_point(buffer, start);
    }

    /// Shows the whole result of the call at `offset` in session `key`'s `*result: NAME*`.
    fn native_result(&mut self, key: SessionKey, offset: usize) {
        let Some(name) = self.hosting.table.get(key).map(|s| s.name.clone()) else {
            return;
        };
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let Some((row, slot, _)) = at(native, offset) else {
            return self.editor.message("No tool call here");
        };
        let Some(tool) = native
            .rows
            .rows()
            .get(row)
            .and_then(|row| point::call(row, slot))
        else {
            let several =
                matches!(&native.rows.rows()[row].kind, Kind::Tools(tools) if tools.len() > 1);
            return self.editor.message(if several {
                "RET on one of its calls; TAB opens the row"
            } else {
                "No tool call here"
            });
        };
        // A result once in never changes: the same call's is shown as it is.
        let kept = native.result.filter(|id| self.editor.buffer(*id).is_some());
        if kept.is_some() && native.result_of.as_deref() == Some(tool.id.as_str()) {
            if let Some(id) = kept {
                self.editor.show(id);
            }
            return;
        }
        let Some(text) = point::whole(tool, native.rows.directory()) else {
            return self.editor.message("No result yet");
        };
        let id = tool.id.clone();
        let shown = show_withheld(
            &mut self.editor,
            &mut native.result,
            &format!("*result: {name}*"),
            &text,
        );
        native.result_of = shown.then_some(id);
    }

    /// Moves point `count` rows of `wanted` on from `offset`, back if negative.
    fn native_move(&mut self, key: SessionKey, offset: usize, count: isize, wanted: Wanted) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let point = match native.placed.row_at(offset) {
            Some(row) => Point::In {
                row,
                first_line: native.placed.start(row) == offset,
            },
            None => Point::End,
        };
        let Some(row) = point::step(native.rows.rows(), point, count, wanted) else {
            let (what, way) = match wanted {
                Wanted::Prompt => ("message", if count < 0 { "earlier" } else { "later" }),
                Wanted::Tools => ("tool row", if count < 0 { "earlier" } else { "later" }),
            };
            return self.editor.message(format!("No {way} {what}"));
        };
        let (buffer, start) = (native.buffer, native.placed.start(row));
        self.native_point(buffer, start);
    }

    /// Puts point at `offset` of `buffer`, when the selected window shows it.
    fn native_point(&mut self, buffer: BufferId, offset: usize) {
        if self.editor.window().buffer() != buffer {
            return;
        }
        let offset = offset.min(
            self.editor
                .buffer(buffer)
                .map_or(0, |b: &Buffer| b.text().len()),
        );
        let selections = &mut self.editor.current_mut().0.selections;
        selections.keep_primary();
        selections.move_primary(offset);
        self.editor.follow_cursor();
    }
}

/// The row at `offset`, the call of an open row of several that it is on, and where that
/// call, or else the row, starts.
fn at(native: &Native, offset: usize) -> Option<(usize, Option<usize>, usize)> {
    let row = native.placed.row_at(offset)?;
    let start = native.placed.start(row);
    let shown = native.rows.rows().get(row)?;
    let several = matches!(&shown.kind, Kind::Tools(tools) if tools.len() > 1);
    if !several || shown.folded {
        return Some((row, None, start));
    }
    let calls = native.calls.get(&row).map_or(&[][..], Vec::as_slice);
    let slot = point::slot(calls, offset - start);
    let call_start = slot
        .and_then(|slot| calls.iter().find(|(s, _)| *s == slot))
        .map_or(start, |(_, range)| start + range.start);
    Some((row, slot, call_start))
}
