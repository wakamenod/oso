//! The messages being sent to a native session (ADR 0165): drawn at its transcript's end as oso
//! writes them, before the CLI replays them, which makes the row that takes their place; at its
//! process's end, the newest of the person's goes back to the composer.
//!
//! Must not depend on: key presses or how a frame is drawn; what is said of a message not
//! confirmed is the rows'.

use oso_core::editor::Editor;

use super::{Native, draw, put_tail};
use crate::app::App;
use crate::app::sessions::empty_composer;
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
    /// person's text whose turn was under way goes back to their composer, if that is empty.
    pub(super) fn sending_ended(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        if native.rows.sending().len() == 0 {
            return;
        }
        let text = native.rows.sending_back().map(str::to_owned);
        let text = text.filter(|_| self.composer_takes_back(key));
        self.editor.refill_wraps();
        if let Some(text) = &text {
            self.set_composer_text(key, text);
        }
        let back = text.is_some() && !self.composer_empty(key);
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            let changes = native.rows.sending_ended(back);
            draw(&mut self.editor, native, changes);
        }
    }

    /// Whether native session `key`'s composer is empty and takes a text back: not where a queued
    /// message is edited, nor one whose mode the person changed or that a yank turned secret.
    fn composer_takes_back(&self, key: SessionKey) -> bool {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return false;
        };
        let buffer = (native.composing.buffer).and_then(|id| self.editor.buffer(id));
        buffer.is_none_or(empty_composer) && !native.composing.edits()
    }
}

#[cfg(test)]
mod tests;
