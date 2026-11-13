//! The menus in force (ADR 0117): the built-in set, the one the configuration gave, which goes
//! in with the user's keymap that names its menus, one that waits for a pending prefix key, and
//! the sets replaced for the caller to drop elsewhere. The open menus live here too, so that
//! what the float is kept by changes as they do.
//!
//! Must not depend on: `app` or winit.

use std::sync::Arc;

use oso_core::key::KeyPress;
use oso_core::menu::Menus;

use super::Stack;
use crate::dispatch::Dispatcher;

/// The menus in force, those given while a prefix key was pending, and the open ones.
#[derive(Debug, Default)]
pub(in crate::dispatch) struct MenuSet {
    /// The built-in menus, which apply with no configuration file.
    builtin: Arc<Menus>,
    pub(super) menus: Arc<Menus>,
    /// The set given with a user's keymap that waits for a prefix key, which goes in with it.
    waiting: Option<Arc<Menus>>,
    /// Sets that ones waiting replaced, for the caller to drop elsewhere.
    replaced: Vec<Arc<Menus>>,
    /// The open menus, if any.
    pub(super) open: Option<Stack>,
    /// How many times a menu has opened, closed or been pushed over another: what the float's
    /// layout is kept by.
    pub(super) changes: u64,
}

impl MenuSet {
    /// The built-in menus, in force.
    pub(in crate::dispatch) fn new(builtin: Menus) -> Self {
        let builtin = Arc::new(builtin);
        Self {
            menus: Arc::clone(&builtin),
            builtin,
            ..Self::default()
        }
    }

    /// Puts in the set that waited, as the user's keymap that waited goes in.
    pub(in crate::dispatch) fn take_waiting(&mut self) {
        if let Some(menus) = self.waiting.take() {
            let old = std::mem::replace(&mut self.menus, menus);
            self.replaced.push(old);
        }
    }

    /// The keys typed in the open menu that begin a head's, if a menu is open.
    pub(in crate::dispatch) fn pending(&self) -> Option<&[KeyPress]> {
        self.open.as_ref().map(|stack| &stack.pending[..])
    }

    /// Whether a menu is open.
    pub(in crate::dispatch) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Opens `stack` in place of any menus open.
    pub(super) fn open(&mut self, stack: Stack) {
        self.open = Some(stack);
        self.changes += 1;
    }

    /// Closes every open menu.
    pub(super) fn close(&mut self) {
        if self.open.take().is_some() {
            self.changes += 1;
        }
    }
}

impl Dispatcher {
    /// Puts in the menus from the configuration, or the built-in ones for `None`, in place of
    /// those before. While a prefix key is pending they wait for it, as
    /// [`Dispatcher::set_user_keymap`]'s keymap does, so that a key always names a menu of the
    /// set its keymap was built with. Give them with the keymap. A menu open now keeps the set
    /// it came from (ADR 0117).
    ///
    /// Returns the set this replaces now, the one in force or one that was waiting, for the
    /// caller to drop elsewhere. One that a waiting set replaces later is kept for
    /// [`Dispatcher::take_replaced_menus`].
    pub fn set_menus(&mut self, menus: Option<Menus>) -> Option<Arc<Menus>> {
        let set = &mut self.menus;
        let menus = menus.map_or_else(|| Arc::clone(&set.builtin), Arc::new);
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
    pub fn take_replaced_menus(&mut self) -> Option<Arc<Menus>> {
        self.menus.replaced.pop()
    }

    /// The menus in force.
    pub(in crate::dispatch) fn menus_in_force(&self) -> &Menus {
        &self.menus.menus
    }

    /// The menus in force.
    #[cfg(test)]
    pub fn menus(&self) -> &Menus {
        self.menus_in_force()
    }
}
