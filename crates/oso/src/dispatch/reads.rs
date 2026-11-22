//! What the dispatcher is reading now: a prefix key, a prefix argument, or text as typed.
//!
//! Must not depend on: `app` or winit.

use oso_core::editor::Editor;

use super::Dispatcher;

/// What the dispatcher is reading now: a prefix key, a prefix argument, or text as typed.
impl Dispatcher {
    /// The keys of the pending prefix, empty if none is pending.
    #[cfg(any(test, feature = "dev"))]
    pub fn pending(&self) -> &[oso_core::key::KeyPress] {
        self.resolver.pending()
    }

    /// The keys of the pending prefix a restart carries: none while `describe-key` reads, as
    /// its keys run nothing and the next instance would run them as a prefix (ADR 0150).
    pub fn pending_to_carry(&self) -> &[oso_core::key::KeyPress] {
        if self.describing() {
            return &[];
        }
        self.resolver.pending()
    }

    /// Whether keys are in progress at all: a prefix key, or a prefix argument, even one that
    /// takes no more digits, which the next command would take.
    pub fn keys_in_progress(&self) -> bool {
        !self.resolver.pending().is_empty() || self.prefix.is_some()
    }

    /// Whether the next key is read as a key rather than typed: a prefix key is
    /// pending, a prefix argument reads digits or a minus, a menu is open, or `describe-key`
    /// reads (ADR 0150). The input method stays off then, as a letter or a digit would start a
    /// composition (ADR 0069, ADR 0117).
    pub fn reads_keys(&self) -> bool {
        self.menu_open()
            || self.describing()
            || !self.resolver.pending().is_empty()
            || self
                .prefix
                .is_some_and(|prefix| prefix.reads_digits() || prefix.reads_minus())
    }

    /// Whether text would go in as typed now: no prefix key or prefix argument waits, no
    /// question or menu is open, `describe-key` reads nothing, and what is active takes text. A commit the window layer makes
    /// unasked waits otherwise, so that it neither takes a prefix argument nor is lost
    /// (ADR 0077).
    pub fn types_text(&self, editor: &Editor) -> bool {
        let question = editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question());
        if question
            || self.prefix.is_some()
            || !self.resolver.pending().is_empty()
            || self.menu_open()
            || self.describing()
        {
            return false;
        }
        let reading = self.keymaps.reading(editor);
        let active = self.keymaps.active(&reading, self.prefix, editor);
        active.text_command().is_some()
    }
}
