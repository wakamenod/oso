//! A native session's earlier turns (ADR 0165, step 5, the author's Q5-1 to Q5-10): at a
//! native start whose rows hold none of its conversation, a resume in a new instance, after
//! `oso-restart` or a fork, the start thread reads them from Claude Code's transcript and hands
//! them on before its stream; here they are drawn as rows, as live turns are, after a note
//! that says they are as the transcript records them and what was left out before them.
//!
//! Must not depend on: key presses or how a frame is drawn; the start thread reads the file.
//!
//! - Rows that hold the conversation read what follows their newest messages, when it may
//!   have gone on elsewhere since: after a stop, or the TUI's turns once a session comes back
//!   from it (part 5b); not after an interrupt's or a mode's resume, which oso made at once.
//! - The notes said while they are read, such as "Resumed.", wait and are drawn after them,
//!   so that the rows read in the order things happened. A start that does not run draws
//!   them at once.
//! - The rows they add are drawn in one change of the buffer, so that thousands of them cost
//!   one edit of it, not one each.
//! - A whole reply among them is highlighted as a live one is (part 2g), the newest first.
//! - A transcript that is missing or does not read leaves the session starting as it would
//!   have, and one note says why.

use oso_core::editor::Editor;

use super::{App, Native, draw};
use crate::sessions::identity::SessionId;
use crate::sessions::native::past::{Past, PastAsk, Since};
use crate::sessions::native::render;
use crate::sessions::start::NativeOrder;
use crate::sessions::worker::SessionKey;

impl App {
    /// What native session `key`'s start reads of conversation `id`: all of it when its rows
    /// hold none of it; what follows their newest messages when it may have gone on elsewhere
    /// since (part 5b); else nothing. Then the notes said meanwhile wait for what is read.
    pub(super) fn native_wants_past(&mut self, key: SessionKey, id: &SessionId) -> Option<PastAsk> {
        let from = self.hosting.table.started_by(id);
        let native = self.hosting.natives.map.get_mut(&key)?;
        let after = match native.rows.holds_conversation() {
            false => Vec::new(),
            true if native.may_have_gone_on => {
                Some(native.rows.kept_ids()).filter(|ids| !ids.is_empty())?
            }
            true => return None,
        };
        native.notes_after_past.get_or_insert_with(Vec::new);
        native.rows.note_started_by(from);
        Some(PastAsk {
            id: id.clone(),
            after,
        })
    }

    /// Native fork `key`'s `order`, which reads the earlier turns of the conversation `from`,
    /// named `name`, that it copies, and says it was forked after them (Q5-5).
    pub(in crate::app::sessions::hosting) fn native_fork_order(
        &mut self,
        key: SessionKey,
        from: &SessionId,
        name: &str,
        order: NativeOrder,
    ) -> NativeOrder {
        let past = self.native_wants_past(key, from);
        self.native_note_row(key, &format!("Forked from {name}."));
        NativeOrder { past, ..order }
    }

    /// Notes whether native session `key`'s conversation may have gone on elsewhere since its
    /// rows, which its next start then reads the turns after (part 5b).
    pub(in crate::app::sessions::hosting) fn native_may_have_gone_on(
        &mut self,
        key: SessionKey,
        gone_on: bool,
    ) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.may_have_gone_on = gone_on;
        }
    }

    /// Whether native session `key` waits for its earlier turns: then `text`, a note of oso's,
    /// waits with them.
    pub(super) fn native_note_waits(&mut self, key: SessionKey, text: &str) -> bool {
        let waiting = (self.hosting.natives.map.get_mut(&key))
            .and_then(|native| native.notes_after_past.as_mut());
        let Some(waiting) = waiting else {
            return false;
        };
        waiting.push(text.to_owned());
        true
    }

    /// The start thread read native session `key`'s earlier turns: drawn, after the note on
    /// what they are and before the notes on what did not read and the notes that waited.
    /// Nothing if it waits for none.
    pub(super) fn native_past(&mut self, key: SessionKey, past: Past) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let Some(waiting) = native.notes_after_past.take() else {
            return;
        };
        // Read: what follows is the stream's. Not read, it is asked again at the next start.
        if past.missing.is_none() {
            native.may_have_gone_on = false;
        }
        let from = native.rows.rows().len();
        if let Some(before) = past.before() {
            native.rows.note(&before);
        }
        let notes = past.after();
        // Whether the conversation's first message is among them.
        let first = !past.cut && !matches!(past.since, Since::After { .. });
        let changes = native.rows.past(past.said, first);
        for note in notes.iter().chain(&waiting) {
            native.rows.note(note);
        }
        // The rows added are drawn in one change; a result that landed on an earlier call's row
        // (part 5b) is drawn where that row is.
        let earlier: Vec<_> = (changes.into_iter())
            .filter(|change| change.row() < from)
            .collect();
        self.editor.refill_wraps();
        draw(&mut self.editor, native, earlier);
        draw_added(&mut self.editor, native, from);
        let buffer = native.buffer;
        if (self.editor.windows()).any(|window| window.buffer() == buffer) {
            self.request_redraw();
        }
    }

    /// Native session `key`'s start did not run: the notes that waited for its earlier turns
    /// are drawn now.
    pub(super) fn native_past_given_up(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let Some(waiting) = native.notes_after_past.take() else {
            return;
        };
        let from = native.rows.rows().len();
        for note in &waiting {
            native.rows.note(note);
        }
        draw_added(&mut self.editor, native, from);
    }
}

/// Draws `native`'s rows from `from` on, each added at the end since it was last drawn, in
/// one change of its buffer.
pub(super) fn draw_added(editor: &mut Editor, native: &mut Native, from: usize) {
    let rows = native.rows.rows();
    if from >= rows.len()
        || native
            .placed
            .range(super::RowChange::Added(from), |_, _| 0)
            .is_none()
    {
        return;
    }
    let start = native.placed.end();
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut lengths = Vec::with_capacity(rows.len() - from);
    // What the tables so far took.
    let mut cells = 0;
    for (at, row) in rows.iter().enumerate().skip(from) {
        let width = (native.widths).of_past(row, cells, native.rows.width());
        let rendered = render::render(row, native.rows.directory(), width);
        if rendered.calls.is_empty() {
            native.calls.remove(&at);
        } else {
            native.calls.insert(at, rendered.calls.clone());
        }
        (native.widths).drawn(at, row, &rendered, false, width);
        cells += rendered.cells;
        let row_runs = rendered.runs();
        // Each row's runs end at its last face: the rest of it is plain.
        let covered: usize = row_runs.iter().map(|(length, _)| length).sum();
        runs.extend(row_runs);
        if rendered.text.len() > covered {
            runs.push((rendered.text.len() - covered, None));
        }
        text.push_str(&rendered.text);
        lengths.push(rendered.text.len());
        native.markdown.drawn(at);
    }
    editor.replace_native_rows(native.buffer, start..start, &text, &runs);
    let mut at = start;
    for (row, length) in (from..).zip(lengths) {
        native.placed.put(row, at..at, length);
        at += length;
    }
    debug_assert_eq!(
        editor.buffer(native.buffer).map(|b| b.text().len()),
        Some(native.placed.end() + native.composing.rows.len())
    );
}
