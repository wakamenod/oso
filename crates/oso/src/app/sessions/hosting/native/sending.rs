//! The messages being sent to a native session (ADR 0165): drawn at its transcript's end as oso
//! writes them, before the CLI replays them, which makes the row that takes their place; at its
//! process's end, the newest of the person's goes back to the composer.
//!
//! Must not depend on: key presses or how a frame is drawn; what is said of a message not
//! confirmed is the rows'.

use oso_core::buffer::Replacement;
use oso_core::editor::Editor;

use super::{Native, draw, put_tail};
use crate::app::App;
use crate::sessions::native::render::{Rendered, sending_rows};
use crate::sessions::native::tail::Part;
use crate::sessions::worker::SessionKey;

/// Of what the messages being sent were drawn last: their version and whether a blank line set
/// them apart; `None` for none drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Drawn(Option<(u64, bool)>);

/// Draws `native`'s messages being sent if they or the row before changed. Nothing is
/// shown before its process runs, as none was written to it. Whether it drew.
pub(super) fn sync(editor: &mut Editor, native: &mut Native) -> bool {
    let shown = native.rows.sending().len() > 0 && !native.starting();
    let version = native.rows.sending_version();
    let now = Drawn(shown.then(|| (version, native.rows.gap_next())));
    // Its buffer may be killed, and the session not swept yet.
    if native.sending == now || editor.buffer(native.buffer).is_none() {
        return false;
    }
    native.sending = now;
    let rendered = match shown {
        true => sending_rows(&native.rows),
        false => Rendered::default(),
    };
    let runs = rendered.runs();
    put_tail(editor, native, Part::Sending, rendered.text, runs)
}

/// Before rows are drawn: takes out what `native` drew of messages being sent that are others now,
/// so that the row of one takes its place and the view of a window at the end does not go past it.
pub(super) fn before_rows(editor: &mut Editor, native: &mut Native) {
    let version = native.rows.sending_version();
    let others = (native.sending.0).is_some_and(|(drawn, ..)| drawn != version);
    if others && editor.buffer(native.buffer).is_some() {
        native.sending = Drawn::default();
        put_tail(editor, native, Part::Sending, String::new(), Vec::new());
    }
}

impl App {
    /// Native session `key`'s process ended: what was being sent is a note that says so, and the
    /// person's text whose turn was under way goes back to their composer, if that is free.
    pub(super) fn sending_ended(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        if native.rows.sending().len() == 0 {
            return;
        }
        let free = self.composer_free(key);
        if free {
            self.composer_back(key);
        }
        self.editor.refill_wraps();
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            end(&mut self.editor, native, free);
        }
    }
}

/// `native`'s process ended: what was being sent is a note, and what goes back is in its composer,
/// if that is `free`.
fn end(editor: &mut Editor, native: &mut Native, free: bool) {
    let composer = native.composing.buffer.filter(|_| free);
    let back = match (composer, native.rows.sending_back()) {
        (Some(composer), Some(text)) => {
            let edit = Replacement { range: 0..0, text };
            editor.apply_edits(composer, &[edit]).is_ok()
        }
        _ => false,
    };
    let changes = native.rows.sending_ended(back);
    draw(editor, native, changes);
}

#[cfg(test)]
mod tests;
