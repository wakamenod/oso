//! The keymaps a dispatcher holds, and which are active for the editor's state: the global
//! one, a prefix argument's, the emergency copies' and the prompts' (ADR 0034, ADR 0072,
//! ADR 0076).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::buffer::Mode;
use oso_core::command::Prefix;
use oso_core::editor::Editor;
use oso_core::keymap::{Keymap, Layers};
use oso_core::minibuffer::Kind;

/// Every keymap a dispatcher holds.
pub(super) struct Keymaps {
    pub(super) global: Keymap,
    /// Active while the prefix argument reads digits, and a minus sign. They come
    /// first among the minor modes, as what is being typed is for them.
    pub(super) prefix: [Keymap; 2],
    /// The keys of the list of emergency copies, and of a buffer showing one (ADR 0064).
    pub(super) copy: [Keymap; 2],
    /// The keys of a prompt that reads an answer, above the global ones (ADR 0072).
    pub(super) minibuffer: Keymap,
    /// The keys of a question, in place of the global ones.
    pub(super) question: Keymap,
    /// The keys of a prompt that completes, with the minibuffer's, above the global ones
    /// (ADR 0076).
    pub(super) completing: Keymap,
}

impl Keymaps {
    /// Every keymap, active or not. Each field is named here, so that a keymap added to the
    /// struct cannot be left out of what `M-x` judges commands by.
    pub(super) fn all(&self) -> [&Keymap; 8] {
        let Self {
            global,
            prefix: [digits, minus],
            copy: [list, copy],
            minibuffer,
            question,
            completing,
        } = self;
        [
            global, digits, minus, list, copy, minibuffer, question, completing,
        ]
    }

    /// The prefix argument's keymaps, as [`Keymaps::active`] takes them.
    pub(super) fn reading(&self) -> [&Keymap; 2] {
        [&self.prefix[0], &self.prefix[1]]
    }

    /// The keymaps active now. A question's keys take the global keymap's place, so that
    /// no other key reaches a command, and a prompt's go above it (ADR 0072). `reading`
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
            return Layers::global(&self.question);
        }
        let minor: &[&Keymap] = match prefix {
            Some(prefix) if prefix.reads_minus() => reading,
            Some(prefix) if prefix.reads_digits() => &reading[..1],
            _ => &[],
        };
        let major = if let Some(prompt) = editor.prompt() {
            match prompt.kind() {
                Kind::Complete { .. } => Some(&self.completing),
                _ => Some(&self.minibuffer),
            }
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
