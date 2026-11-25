//! A native session's composer and the history of what the person sent (ADR 0165, step 2,
//! "History"): what goes into it, and `M-p` and `M-n` over it. Its rules are
//! `sessions::prompt_history`'s, and `native/prompt_history.rs` holds it.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - A walk keeps the draft it started from, which `M-n` past the newest gives back. It ends
//!   there, and as what is in the composer goes another way: sent, a queued message's edit in
//!   it, or the composer gone.
//! - A composer a yank turned secret takes no part: no draft of it is kept.

use std::sync::Arc;

use oso_core::buffer::{BufferId, Replacement};

use super::composed::{refuses, too_much};
use crate::app::App;
use crate::sessions::native::queue::MAX_MESSAGE;
use crate::sessions::prompt_history::Step;
use crate::sessions::worker::SessionKey;

/// Where `M-p` is: when the message shown was sent, and the draft the walk started from.
#[derive(Default)]
pub(super) struct Walk {
    shown: u64,
    draft: String,
}

impl std::fmt::Debug for Walk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Walk").finish_non_exhaustive()
    }
}

impl App {
    /// The person sent `text` from native session `key`'s composer, now or queued: the
    /// composer is empty, `text` goes first in the history, and a walk there ends.
    pub(super) fn sent_from_composer(&mut self, key: SessionKey, text: &str) {
        self.set_composer_text(key, "");
        self.end_walk(key);
        self.note_prompt(text);
    }

    /// Native session `key`'s walk ends, its draft let go.
    pub(super) fn end_walk(&mut self, key: SessionKey) {
        if let Some(native) = self.hosting.natives.map.get_mut(&key) {
            native.composing.walk = None;
        }
    }

    /// Native session `key`'s composer, if it may take the person's past words.
    fn composer_for_past(&mut self, key: SessionKey) -> Result<BufferId, String> {
        let id = (self.composer(key)).ok_or_else(|| "The session has no composer".to_owned())?;
        let buffer = (self.editor.buffer(id)).ok_or_else(|| "The composer is gone".to_owned())?;
        refuses(buffer).map(|()| id)
    }

    /// `M-p` and `M-n`: the message sent `count` before the one shown, after if negative, in
    /// any native session; past the newest, the draft the walk started from. `M-n` in an empty
    /// composer that walks nowhere puts the suggestion in (part 3f).
    pub(super) fn composer_history(&mut self, key: SessionKey, count: isize) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let from = native.composing.walk.as_ref().map(|walk| walk.shown);
        if count < 0 && from.is_none() && self.take_suggestion(key) {
            return;
        }
        if let Err(why) = self.walk(key, from, count) {
            self.editor.message(why);
        }
    }

    /// A step of the walk of native session `key`, which is at `from`; or why none.
    fn walk(&mut self, key: SessionKey, from: Option<u64>, count: isize) -> Result<(), String> {
        let id = self.composer_for_past(key)?;
        if self.hosting.natives.prompts.is_empty() && from.is_none() {
            return Err("Nothing sent yet".to_owned());
        }
        let to = match self.hosting.natives.prompts.step(from, count) {
            Step::Entry(entry) => Some((entry.sent, Arc::clone(&entry.text))),
            Step::Start => None,
        };
        let walk = (self.hosting.natives.map.get_mut(&key)).map(|n| &mut n.composing.walk);
        let Some(walk) = walk else {
            return Ok(());
        };
        match (to, walk) {
            (Some((sent, text)), Some(walk)) => {
                walk.shown = sent;
                self.set_composer_text(key, &text);
            }
            (Some((sent, text)), walk) => {
                let draft = self.editor.buffer(id).map(|buffer| buffer.text());
                // Measured before it is copied.
                if draft.is_some_and(|draft| draft.len() > MAX_MESSAGE) {
                    return Err("The message is over 64 KiB: it would not be kept".to_owned());
                }
                let draft = draft.map(|draft| draft.to_string()).unwrap_or_default();
                *walk = Some(Walk { shown: sent, draft });
                self.set_composer_text(key, &text);
            }
            (None, walk) => {
                if let Some(Walk { draft, .. }) = walk.take() {
                    self.set_composer_text(key, &draft);
                }
            }
        }
        Ok(())
    }

    /// `C-c C-r`: the past prompt chosen goes into native session `key`'s composer at point,
    /// as it was sent, not sent: it is the person's own words, as a yank's are.
    pub(super) fn put_past_prompt(&mut self, key: SessionKey) {
        let put = match self.editor.take_past_prompt() {
            Some(text) => self.put_past(key, &text),
            None => Err("Nothing was put in".to_owned()),
        };
        if let Err(why) = put {
            self.editor.message(why);
        }
    }

    /// Puts `text` in native session `key`'s composer at point, or says why not.
    fn put_past(&mut self, key: SessionKey, text: &str) -> Result<(), String> {
        let id = self.composer_for_past(key)?;
        if self.editor.current().0.buffer() != id {
            return Err("Nothing was put in".to_owned());
        }
        let len = (self.editor.buffer(id)).map_or(0, |buffer| buffer.text().len());
        if len.saturating_add(text.len()) > MAX_MESSAGE {
            return Err(too_much());
        }
        self.composer_back(key);
        let at = self.editor.current().0.selections.primary().cursor;
        let edit = Replacement {
            range: at..at,
            text,
        };
        (self.editor.apply_edits(id, &[edit])).map_err(|_| "Nothing was put in".to_owned())?;
        let selections = &mut self.editor.current_mut().0.selections;
        selections.keep_primary();
        selections.move_primary(at + text.len());
        self.editor.refill_wraps();
        self.editor.follow_cursor();
        Ok(())
    }
}
