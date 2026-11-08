//! The keymaps a dispatcher holds, and which of them are active at a moment: a prefix
//! argument's, a prompt's or a question's, isearch's, a mode's and the global one
//! (ADR 0034, ADR 0072, ADR 0082).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::buffer::Mode;
use oso_core::command::Prefix;
use oso_core::editor::Editor;
use oso_core::keymap::{Keymap, Layers};

/// Every keymap a dispatcher holds.
pub(super) struct Keymaps {
    pub(super) global: Keymap,
    /// Active while the prefix argument reads digits, and a minus sign. They come
    /// first among the minor modes, as what is being typed is for them.
    pub(super) prefix: [Keymap; 2],
    /// The keys of the list of emergency copies, and of a buffer showing one (ADR 0064).
    pub(super) copy: [Keymap; 2],
    /// The keys of a prompt that reads an answer, above the global ones, and of a
    /// question, in place of them (ADR 0072).
    pub(super) prompt: [Keymap; 2],
    /// The keys of a search under way, in place of the others (ADR 0082).
    pub(super) isearch: Keymap,
}

impl Keymaps {
    /// The prefix argument's keymaps, as [`Keymaps::active`] takes them.
    pub(super) fn reading(&self) -> [&Keymap; 2] {
        [&self.prefix[0], &self.prefix[1]]
    }

    /// The keymaps active now. A question's keys take the global keymap's place, so that
    /// no other key reaches a command, and a prompt's go above it (ADR 0072). So do
    /// isearch's, and a key they do not bind ends the search first (ADR 0082). `reading`
    /// holds the keymaps of a prefix argument.
    pub(super) fn active<'a>(
        &'a self,
        reading: &'a [&'a Keymap; 2],
        prefix: Option<Prefix>,
        editor: &Editor,
    ) -> Layers<'a> {
        if editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question())
        {
            return Layers::global(&self.prompt[1]);
        }
        if editor.isearch().is_some() {
            return Layers::global(&self.isearch);
        }
        let minor: &[&Keymap] = match prefix {
            Some(prefix) if prefix.reads_minus() => reading,
            Some(prefix) if prefix.reads_digits() => &reading[..1],
            _ => &[],
        };
        let major = if editor.prompt().is_some() {
            Some(&self.prompt[0])
        } else {
            match editor.current().1.mode() {
                Mode::Fundamental => None,
                Mode::CopyList => Some(&self.copy[0]),
                Mode::CopyView => Some(&self.copy[1]),
            }
        };
        Layers {
            minor,
            major,
            ..Layers::global(&self.global)
        }
    }
}
