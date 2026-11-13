//! The menus a key may open (ADR 0117): the set in force, which goes in with the user's keymap
//! that names its menus, and what a key bound to a menu does. Opening one, its stack and its
//! rules come with the float (step 2); until then such a key only says so.
//!
//! Must not depend on: `app` or winit.

use oso_core::command::Effects;
use oso_core::editor::Editor;
use oso_core::menu::{MenuId, Menus};

use super::Dispatcher;

/// The menus in force, and those given while a prefix key was pending.
#[derive(Debug, Default)]
pub(super) struct MenuSet {
    /// The built-in menus, which apply with no configuration file.
    builtin: Menus,
    menus: Menus,
    /// The set given with a user's keymap that waits for a prefix key, which goes in with it.
    waiting: Option<Menus>,
    /// Sets that ones waiting replaced, for the caller to drop elsewhere.
    replaced: Vec<Menus>,
}

impl MenuSet {
    /// The built-in menus, in force.
    pub(super) fn new(builtin: Menus) -> Self {
        Self {
            menus: builtin.clone(),
            builtin,
            waiting: None,
            replaced: Vec::new(),
        }
    }

    /// Puts in the set that waited, as the user's keymap that waited goes in.
    pub(super) fn take_waiting(&mut self) {
        if let Some(menus) = self.waiting.take() {
            let old = std::mem::replace(&mut self.menus, menus);
            self.replaced.push(old);
        }
    }
}

impl Dispatcher {
    /// Puts in the menus from the configuration, or the built-in ones for `None`, in place of
    /// those before. While a prefix key is pending they wait for it, as
    /// [`Dispatcher::set_user_keymap`]'s keymap does, so that a key always names a menu of the
    /// set its keymap was built with. Give them with the keymap.
    ///
    /// Returns the set this replaces now, the one in force or one that was waiting, for the
    /// caller to drop elsewhere. One that a waiting set replaces later is kept for
    /// [`Dispatcher::take_replaced_menus`].
    pub fn set_menus(&mut self, menus: Option<Menus>) -> Option<Menus> {
        let set = &mut self.menus;
        let menus = menus.unwrap_or_else(|| set.builtin.clone());
        if self.resolver.pending().is_empty() {
            if let Some(stale) = set.waiting.take() {
                set.replaced.push(stale);
            }
            Some(std::mem::replace(&mut set.menus, menus))
        } else {
            set.waiting.replace(menus)
        }
    }

    /// A set of menus that a waiting one replaced, once its prefix key resolved, for the caller
    /// to drop elsewhere. Call it until it gives `None`.
    pub fn take_replaced_menus(&mut self) -> Option<Menus> {
        self.menus.replaced.pop()
    }

    /// The menus in force.
    #[cfg(test)]
    pub fn menus(&self) -> &Menus {
        &self.menus.menus
    }

    /// A key bound to `menu` was pressed. Menus open with their float (ADR 0117 step 2); until
    /// then it says so, as an undefined key says what it is.
    pub(super) fn menu_key(&mut self, editor: &mut Editor, menu: MenuId) -> Effects {
        match self.menus.menus.get(menu) {
            Some(definition) => {
                log::debug!("menu {} cannot open yet", definition.name());
                editor.echo("Menus cannot open yet");
            }
            // The configuration unbinds a default key whose menu it removes, so this is a menu
            // of no set in force.
            None => {
                log::debug!("a key names a menu no longer defined");
                editor.echo("That key is undefined");
            }
        }
        self.forget_keys(editor);
        Effects::default()
    }
}
