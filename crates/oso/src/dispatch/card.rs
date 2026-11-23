//! A key typed in a native session's empty composer that answers the session's permission card
//! (ADR 0180): the host says, before it hands the key over, that the key would answer now, and
//! the card's command for it runs in place of typing it.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.
//!
//! What the host said holds for that key alone, and only where it would be typed: a key a user's
//! keymap binds, one after a prefix key or a prefix argument, and one read by a prompt, a search
//! or a menu does what it does.

use oso_core::buffer::Mode;
use oso_core::command::CommandId;
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::Binding;

use super::Dispatcher;

impl Dispatcher {
    /// Whether the key handled next answers a native session's card from its composer, as the
    /// host judged it just before; the host takes it back once the key was handled.
    pub fn set_key_answers_card(&mut self, answers: bool) {
        self.key_answers_card = answers;
    }

    /// Takes back what the host said of the key just handled, and says whether the key ran the
    /// card's command in place of being typed.
    pub fn take_key_answered_card(&mut self) -> bool {
        self.key_answers_card = false;
        std::mem::take(&mut self.key_answered_card)
    }

    /// The command to run in place of `id`, which `key` resolved to: the card's for `key`, if it
    /// would be typed in an empty composer and the host said it answers the card.
    pub(super) fn card_in_place_of(
        &mut self,
        editor: &Editor,
        id: CommandId,
        key: KeyPress,
    ) -> CommandId {
        if !std::mem::take(&mut self.key_answers_card)
            || self.prefix.is_some()
            || !self.persons_typing(editor, id)
            || editor.prompt().is_some()
            || editor.macro_replaying()
        {
            return id;
        }
        let window = editor.window();
        let empty = window.selections.len() == 1
            && editor
                .buffer(window.buffer())
                .is_some_and(|buffer| buffer.mode() == Mode::Composer && buffer.text().is_empty());
        match self.keymaps.card().get(key) {
            Some(Binding::Command(card)) if empty => {
                self.key_answered_card = true;
                card
            }
            _ => id,
        }
    }
}
