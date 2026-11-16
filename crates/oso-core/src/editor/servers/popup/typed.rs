//! What the person typed, for the host to judge whether completions are asked for by themselves
//! (ADR 0138, Completion in the buffer, step 9b): only where it was typed, noted once a key;
//! the character typed and the word before the cursor are read only once the host knows a
//! server completes there.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O.
//!
//! Only the person's own typing is noted (the dispatcher's), never the agent's, a replayed
//! macro's or a yank, and never in a buffer that may hold secrets or with a prompt open.

use unicode_segmentation::UnicodeSegmentation;

use super::super::super::Editor;
use super::super::jumps::AskAt;
use super::text::{text_of, word_start};

/// What the person typed: where, and once read, the character before the cursor and how many
/// grapheme clusters the identifier before it has, at most what a word is looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionTyped {
    pub at: AskAt,
    pub last: char,
    pub word_clusters: usize,
}

impl Editor {
    /// The person typed into the selected window's buffer: noted for the host, unless the buffer
    /// may hold secrets or a prompt is open. The dispatcher calls it after the person's
    /// `self-insert-command` only.
    pub fn note_completion_typed(&mut self) {
        if self.prompt().is_some() || self.current().1.is_secret() {
            return;
        }
        self.servers.typed = Some(self.ask_at());
    }

    /// Where the person last typed, once.
    pub fn take_completion_typed(&mut self) -> Option<AskAt> {
        self.servers.typed.take()
    }

    /// What the person typed at `at`, while nothing moved since: the character before the
    /// cursor and the identifier's clusters before it.
    pub fn completion_typed(&self, at: AskAt) -> Option<CompletionTyped> {
        if self.ask_at() != at {
            return None;
        }
        let text = self.current().1.text();
        let cursor = at.offset;
        let last = text_of(text, cursor.saturating_sub(4)..cursor)
            .chars()
            .next_back()?;
        let start = word_start(text, cursor);
        let word_clusters = text_of(text, start..cursor).graphemes(true).count();
        Some(CompletionTyped {
            at,
            last,
            word_clusters,
        })
    }
}
