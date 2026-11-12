//! The keymaps a dispatcher holds, and which of them are active at a moment: a prefix
//! argument's, a prompt's, a completion's or a question's, isearch's, a mode's, the user's and
//! the global one (ADR 0034, ADR 0072, ADR 0076, ADR 0082, ADR 0108).
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
    /// The user's keys from the configuration, just above the global ones (ADR 0108). Empty
    /// without any, and then not a layer.
    pub(super) user: Keymap,
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
    /// The keys of a search under way, in place of the others (ADR 0082).
    pub(super) isearch: Keymap,
    /// The answers of a query-replace under way, in place of the others (ADR 0113).
    pub(super) replace: Keymap,
}

impl Keymaps {
    /// Every keymap, active or not. Each field is named here, so that a keymap added to the
    /// struct cannot be left out of what `M-x` judges commands by.
    pub(super) fn all(&self) -> [&Keymap; 11] {
        let Self {
            global,
            user,
            prefix: [digits, minus],
            copy: [list, copy],
            minibuffer,
            question,
            completing,
            isearch,
            replace,
        } = self;
        [
            global, user, digits, minus, list, copy, minibuffer, question, completing, isearch,
            replace,
        ]
    }

    /// The prefix argument's keymaps, as [`Keymaps::active`] takes them.
    pub(super) fn reading(&self) -> [&Keymap; 2] {
        [&self.prefix[0], &self.prefix[1]]
    }

    /// The keymaps active now. A question's keys take the global keymap's place, so that
    /// no other key reaches a command, and a prompt's go above it (ADR 0072), a completion's
    /// with them (ADR 0076). So do isearch's, and a key they do not bind ends the search first
    /// (ADR 0082). The user's keys go just above the global ones, so neither a question nor a
    /// search under way sees them (ADR 0108). `reading` holds the keymaps of a prefix argument.
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
        if editor.isearch().is_some() {
            return Layers::global(&self.isearch);
        }
        if editor.replace().is_some() {
            return Layers::global(&self.replace);
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
                Mode::Fundamental | Mode::Rust | Mode::Markdown | Mode::Toml => None,
                Mode::CopyList => Some(&self.copy[0]),
                Mode::CopyView => Some(&self.copy[1]),
            }
        };
        Layers {
            minor,
            major,
            user: self.user(),
            ..Layers::global(&self.global)
        }
    }

    /// The user's keymap as a layer, none when it is empty, so that a key costs nothing more
    /// without user keys.
    pub(super) fn user(&self) -> Option<&Keymap> {
        Some(&self.user).filter(|user| !user.is_empty())
    }
}
