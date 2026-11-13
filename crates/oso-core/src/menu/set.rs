//! The set of menus in force, by name and by id (ADR 0117). Built-in menus are defined first, so
//! their ids are the same in every set built from them, and a default key bound to one names it
//! whatever a configuration file adds, replaces or removes.
//!
//! Must not depend on: `keymap`.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Definition, MAX_MENUS, MenuError, MenuId, is_name};

/// The menus in force. An id keeps its slot when its menu is removed, so a key bound to it finds
/// nothing rather than another menu.
#[derive(Debug, Clone, Default)]
pub struct Menus {
    /// Each id's name and definition, by the id's number; `None` for one reserved or removed.
    slots: Vec<(String, Option<Arc<Definition>>)>,
    by_name: HashMap<String, MenuId>,
}

impl Menus {
    /// No menus.
    pub fn new() -> Self {
        Self::default()
    }

    /// The id for `name`: the one it has, or a new one with no definition yet, so that a head
    /// can name a menu defined after it. Refused for a name that is not lower-case kebab case of
    /// at most 64 bytes, or past 256 ids.
    pub fn id(&mut self, name: &str) -> Result<MenuId, MenuError> {
        if let Some(&id) = self.by_name.get(name) {
            return Ok(id);
        }
        if !is_name(name) {
            return Err(MenuError::BadName);
        }
        if self.slots.len() >= MAX_MENUS {
            return Err(MenuError::TooManyMenus);
        }
        let id = MenuId(u16::try_from(self.slots.len()).map_err(|_| MenuError::TooManyMenus)?);
        self.slots.push((name.to_owned(), None));
        self.by_name.insert(name.to_owned(), id);
        Ok(id)
    }

    /// Defines the menu `id` names as `definition`, which must have its name. Refused if it is
    /// defined already: replacing one whole is [`Menus::remove`], then this.
    pub fn define(&mut self, id: MenuId, definition: Definition) -> Result<(), MenuError> {
        let Some((name, slot)) = self.slots.get_mut(usize::from(id.0)) else {
            return Err(MenuError::WrongName);
        };
        if name != definition.name() {
            return Err(MenuError::WrongName);
        }
        if slot.is_some() {
            return Err(MenuError::NameTaken);
        }
        *slot = Some(Arc::new(definition));
        Ok(())
    }

    /// Removes the menu `id` names, keeping the id, as `[menus.<name>] = false` does.
    pub fn remove(&mut self, id: MenuId) {
        if let Some((_, slot)) = self.slots.get_mut(usize::from(id.0)) {
            *slot = None;
        }
    }

    /// The id of the menu defined as `name`, if one is.
    pub fn lookup(&self, name: &str) -> Option<MenuId> {
        self.by_name
            .get(name)
            .copied()
            .filter(|&id| self.get(id).is_some())
    }

    /// The definition of the menu `id` names, if it is defined. An open menu keeps a clone of
    /// it, so that a reload never changes what is open.
    pub fn get(&self, id: MenuId) -> Option<&Arc<Definition>> {
        self.slots.get(usize::from(id.0))?.1.as_ref()
    }

    /// Every defined menu, in the order of their ids: built-in ones first.
    pub fn iter(&self) -> impl Iterator<Item = (MenuId, &Definition)> {
        self.slots.iter().enumerate().filter_map(|(i, (_, slot))| {
            let id = MenuId(u16::try_from(i).ok()?);
            Some((id, &**slot.as_ref()?))
        })
    }

    /// How many menus are defined.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|(_, slot)| slot.is_some()).count()
    }

    /// Whether none is.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
