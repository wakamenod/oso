//! The keymaps a dispatcher holds, and which of them are active at a moment: a prefix
//! argument's, the layer `C-x TAB` opens, a prompt's, a completion's or a question's, a jump's,
//! isearch's, a mode's, the user's and the global one (ADR 0034, ADR 0072, ADR 0076, ADR 0082,
//! ADR 0108, ADR 0115, ADR 0116).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.
//!
//! The default keymaps come from `bindings`. The user's keymap from the configuration goes in
//! here too, once no prefix key waits for the one before.

use oso_core::buffer::Mode;
use oso_core::command::{CommandId, Prefix, Registry};
use oso_core::editor::Editor;
use oso_core::keymap::{Keymap, Layers};
use oso_core::minibuffer::Kind;

use crate::bindings;

use super::Dispatcher;

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
    /// The keys of a jump under way, in place of the others, isearch's too (ADR 0116).
    pub(super) jump: Keymap,
    /// The answers of a query-replace under way, in place of the others (ADR 0113).
    pub(super) replace: Keymap,
    /// The keys of the layer `C-x TAB` opens, first while it is open (ADR 0115).
    pub(super) rigid: Keymap,
    /// The commands the layer runs rather than closes for.
    pub(super) rigid_commands: Vec<CommandId>,
    /// Markdown's own keys, above the global ones in a Markdown buffer.
    pub(super) markdown: Keymap,
    /// `*Sessions*`'s own keys, above the global ones in it (ADR 0121).
    pub(super) sessions: Keymap,
    /// A review buffer's own keys, above the global ones (ADR 0122).
    pub(super) review: Keymap,
}

impl Keymaps {
    /// The default keymaps over the commands in `registry`, with `global` as the global keymap
    /// and no user keys.
    pub(super) fn new(registry: &Registry, global: Keymap) -> Self {
        let (digits, minus) = bindings::prefix_keymaps(registry);
        let (list, copy) = bindings::recovery_keymaps(registry);
        let [minibuffer, question, completing] = bindings::prompt_keymaps(registry);
        Self {
            global,
            user: Keymap::new(),
            prefix: [digits, minus],
            copy: [list, copy],
            minibuffer,
            question,
            completing,
            isearch: bindings::isearch_keymap(registry),
            jump: bindings::jump_keymap(registry),
            replace: bindings::replace_keymap(registry),
            rigid: bindings::rigid_keymap(registry),
            rigid_commands: bindings::rigid_commands(registry),
            markdown: bindings::markdown_keymap(registry),
            sessions: bindings::sessions_keymap(registry),
            review: bindings::review_keymap(registry),
        }
    }

    /// Every keymap, active or not. Each field is named here, so that a keymap added to the
    /// struct cannot be left out of what `M-x` judges commands by.
    pub(super) fn all(&self) -> [&Keymap; 16] {
        let Self {
            global,
            user,
            prefix: [digits, minus],
            copy: [list, copy],
            minibuffer,
            question,
            completing,
            isearch,
            jump,
            replace,
            rigid,
            rigid_commands: _,
            markdown,
            sessions,
            review,
        } = self;
        [
            global, user, digits, minus, list, copy, minibuffer, question, completing, isearch,
            jump, replace, rigid, markdown, sessions, review,
        ]
    }

    /// The prefix argument's keymaps and the layer of `C-x TAB`, as [`Keymaps::active`] takes
    /// them.
    pub(super) fn reading(&self) -> [&Keymap; 3] {
        [&self.prefix[0], &self.prefix[1], &self.rigid]
    }

    /// The keymaps active now. A question's keys take the global keymap's place, so that
    /// no other key reaches a command, and a prompt's go above it (ADR 0072), a completion's
    /// with them (ADR 0076). So do isearch's, and a key they do not bind ends the search first
    /// (ADR 0082). The user's keys go just above the global ones, so neither a question nor a
    /// search under way sees them (ADR 0108). `reading` holds the keymaps of a prefix argument,
    /// then the layer of `C-x TAB`, first while it is open and no prefix argument reads.
    pub(super) fn active<'a>(
        &'a self,
        reading: &'a [&'a Keymap; 3],
        prefix: Option<Prefix>,
        editor: &Editor,
    ) -> Layers<'a> {
        if editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question())
        {
            return Layers::global(&self.question);
        }
        if editor.jump().is_some() {
            return Layers::global(&self.jump);
        }
        if editor.isearch().is_some() {
            return Layers::global(&self.isearch);
        }
        if editor.replace().is_some() {
            return Layers::global(&self.replace);
        }
        let minor: &[&Keymap] = match prefix {
            Some(prefix) if prefix.reads_minus() => &reading[..2],
            Some(prefix) if prefix.reads_digits() => &reading[..1],
            _ if editor.indent_rigidly_layer() => &reading[2..],
            _ => &[],
        };
        let major = if let Some(prompt) = editor.prompt() {
            match prompt.kind() {
                Kind::Complete { .. } => Some(&self.completing),
                _ => Some(&self.minibuffer),
            }
        } else {
            match editor.current().1.mode() {
                Mode::Fundamental | Mode::Rust | Mode::Toml => None,
                Mode::Review => Some(&self.review),
                Mode::Markdown => Some(&self.markdown),
                Mode::Sessions => Some(&self.sessions),
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

/// The user's keymap from the configuration, which waits for a pending prefix key to go in
/// (ADR 0108).
impl Dispatcher {
    /// Puts in the user's keymap that waited for a prefix key, once none is pending. The one
    /// it replaces waits in turn, for [`Dispatcher::take_replaced_keymap`].
    pub(super) fn take_waiting_user(&mut self) {
        if !self.resolver.pending().is_empty() {
            return;
        }
        self.menus.take_waiting();
        if let Some(user) = self.user_waiting.take() {
            let old = std::mem::replace(&mut self.keymaps.user, user);
            self.replaced.push(old);
        }
    }

    /// Puts the user's keys from the configuration just above the global keymap, in place of
    /// those before; an empty keymap takes them away (ADR 0108). While a prefix key is
    /// pending, they wait for it to resolve, so that its next key reads the keymaps it
    /// started in.
    ///
    /// Returns the keymap this replaces now, the one in force or one that was waiting, for the
    /// caller to drop elsewhere: a large one takes milliseconds to drop. One that a waiting
    /// keymap replaces later is kept for [`Dispatcher::take_replaced_keymap`].
    pub fn set_user_keymap(&mut self, user: Keymap) -> Option<Keymap> {
        if self.resolver.pending().is_empty() {
            if let Some(stale) = self.user_waiting.take() {
                self.replaced.push(stale);
            }
            Some(std::mem::replace(&mut self.keymaps.user, user))
        } else {
            self.user_waiting.replace(user)
        }
    }

    /// A user's keymap that a waiting one replaced, once its prefix key resolved, for the
    /// caller to drop elsewhere. Call it until it gives `None`.
    pub fn take_replaced_keymap(&mut self) -> Option<Keymap> {
        self.replaced.pop()
    }
}
