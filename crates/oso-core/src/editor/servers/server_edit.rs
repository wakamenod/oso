//! A server's workspace edit applied to the buffers it names (ADR 0138, step 13): a code
//! action's now, and a server's own `workspace/applyEdit` (13b) and a rename's (step 14) later.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O; the host
//! places the edits in each buffer's revision and hands them over.
//!
//! - It applies whole or not at all (Q8): every buffer it names must be there, once, may hold no
//!   secrets, be writable and be at the revision the server had, and no two of its edits in one
//!   buffer may overlap. Each buffer's edits are then one undo step.
//! - Nothing of it reaches a log: an edit's text may quote what the server read.

use std::ops::Range;

use crate::buffer::{BufferId, Replacement};
use crate::editor::Editor;

/// One edit: the bytes in its range become its text.
type Edit = (Range<usize>, String);

/// One buffer's part of a server's edit: its edits, in bytes of its revision `revision`, in
/// the server's order.
#[derive(Clone, PartialEq, Eq)]
pub struct BufferEdit {
    pub buffer: BufferId,
    pub revision: u64,
    pub edits: Vec<(Range<usize>, String)>,
}

/// What a server's edit changed: how many edits, in how many buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditApplied {
    pub edits: usize,
    pub buffers: usize,
}

impl Editor {
    /// Applies `edit`, a server's edit of buffers, whole or not at all: each buffer's edits as one
    /// undo step, the cursors moved as the edits move them.
    ///
    /// # Errors
    ///
    /// Why nothing changed, for the echo area: a buffer that is gone, may hold secrets, cannot be
    /// edited or changed since the server's revision, or named twice, or edits that overlap.
    pub fn apply_server_edit(
        &mut self,
        edit: Vec<BufferEdit>,
    ) -> Result<EditApplied, &'static str> {
        let mut checked: Vec<(BufferId, Vec<Edit>)> = Vec::with_capacity(edit.len());
        for part in edit {
            if checked.iter().any(|(id, _)| *id == part.buffer) {
                return Err("It edits a buffer twice");
            }
            let buffer = self
                .buffer(part.buffer)
                .ok_or("A buffer it edits is gone")?;
            if buffer.is_secret() {
                return Err("No server edits a buffer that may hold secrets");
            }
            if buffer.read_only().is_some() {
                return Err("A buffer it edits is read-only");
            }
            if buffer.revision() != part.revision {
                return Err("A buffer it edits changed since the server answered");
            }
            checked.push((part.buffer, joined(part.edits)?));
        }
        let mut applied = EditApplied {
            edits: 0,
            buffers: 0,
        };
        // Nothing failed above, and each buffer was checked writable: each applies.
        for (id, edits) in checked {
            if edits.is_empty() {
                continue;
            }
            let replacements: Vec<Replacement<'_>> = (edits.iter())
                .map(|(range, text)| Replacement {
                    range: range.clone(),
                    text,
                })
                .collect();
            self.apply_edits(id, &replacements)
                .map_err(|_| "A buffer it edits could not be edited")?;
            applied.edits += edits.len();
            applied.buffers += 1;
        }
        Ok(applied)
    }
}

/// `edits` in offset order, the insertions at one place joined in the server's order, with the
/// replacement that starts there, or why they overlap. `Editor::apply_edits` joins insertions at
/// one place too, but in its own order: the protocol's is the server's (LSP `TextEdit[]`).
fn joined(mut edits: Vec<Edit>) -> Result<Vec<Edit>, &'static str> {
    // Stable: insertions at one place keep the server's order.
    edits.sort_by_key(|(range, _)| range.start);
    let mut out: Vec<Edit> = Vec::with_capacity(edits.len());
    for (range, text) in edits {
        if range.end < range.start {
            return Err("Its edits do not read");
        }
        match out.last_mut() {
            Some((last, joined)) if last.start == last.end && last.start == range.start => {
                joined.push_str(&text);
                last.end = range.end;
            }
            Some((last, _)) if range.start < last.end => {
                return Err("Its edits overlap");
            }
            _ => out.push((range, text)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
