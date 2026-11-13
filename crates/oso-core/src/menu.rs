//! Menus (ADR 0117): one component for hydra's heads and transient's switches. A menu is a named
//! key layer with a float that lists it, defined as data, by the built-in tables or by
//! `config.toml`, and checked as it is built.
//!
//! Must not depend on: `keymap`, which binds keys to menus by [`MenuId`], or anything that
//! draws; [`layout`] works out the float's cells and nothing else.
//!
//! - A [`Definition`] has a title, groups of heads laid out as columns, a default for its heads
//!   ([`Stay`]) and a rule for keys it does not bind ([`ForeignKeys`]). Its heads are one list,
//!   and a head is named by its index in it.
//! - A [`Head`] is up to [`MAX_KEYS`] keys, a label and a [`HeadAction`]: a command, a switch,
//!   another menu, or only closing the menu.
//! - [`Menus`] is the set of menus in force, by name and by id. Built-in menus come first, so a
//!   default key names the same menu whatever the file says.
//! - [`Open`] is one open menu with its switches' values; the stack of them is the dispatcher's.
//! - A built-in menu may have a [`Describe`] function, which gives [`Notes`] as it is drawn.

use crate::command::CommandId;
use crate::key::{Key, KeyPress, KeySequence, Modifiers, cancels as is_cancel};

mod definition;
mod error;
mod layout;
mod open;
mod set;

pub use definition::Definition;
pub use error::MenuError;
pub use layout::{Column, Layout, layout};
pub use open::{Describe, Note, Notes, Open};
pub use set::Menus;

/// The most menus a [`Menus`] holds, built-in and configured together.
pub const MAX_MENUS: usize = 256;

/// The most heads in one menu.
pub const MAX_HEADS: usize = 128;

/// The most columns (groups) in one row of a menu.
pub const MAX_COLUMNS: usize = 8;

/// The most rows of columns in one menu: a group's row is 1 to this.
pub const MAX_ROWS: u8 = 3;

/// The most keys in one head's sequence.
pub const MAX_KEYS: usize = 3;

/// The largest fixed count a head gives, either way: `count` is -1000 to 1000.
pub const MAX_COUNT: i16 = 1_000;

/// The most cells in a label or a title.
pub const MAX_LABEL_CELLS: usize = 64;

/// The most bytes in a menu's or a switch's name.
pub const MAX_NAME_BYTES: usize = 64;

/// The most menus open at once, each over the one that opened it.
pub const MAX_DEPTH: usize = 8;

/// Identifies a menu in the [`Menus`] that returned it. Keymaps store these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MenuId(u16);

/// What a head does to its menu once its command has run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stay {
    /// Close the menu, and every menu under it, then run (hydra's blue).
    #[default]
    Exit,
    /// Run and keep the menu open (hydra's red).
    Stay,
}

/// What a key the menu does not bind does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForeignKeys {
    /// Refused, with `q to quit` in the echo area; the menu stays.
    #[default]
    Warn,
    /// Closes the menu and runs as usual, as a key ends isearch.
    Exit,
    /// Runs as usual, and the menu stays (hydra's pink).
    Run,
}

/// What a head does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadAction {
    /// Runs a registered command.
    Command(CommandId),
    /// Flips the open menu's switch of this name; the menu stays.
    Switch(String),
    /// Opens this menu over the current one.
    Menu(MenuId),
    /// Only closes the menu, as hydra's `("q" nil)`.
    Quit,
}

/// One head of a menu, as it is given to [`Definition::add_head`], which checks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// The keys that run it: 1 to [`MAX_KEYS`].
    pub keys: KeySequence,
    /// What the float says for it: at most [`MAX_LABEL_CELLS`] cells.
    pub label: String,
    /// What it does.
    pub action: HeadAction,
    /// Whether the menu stays after it; `None` takes the menu's default.
    pub stay: Option<Stay>,
    /// A fixed count it runs with, as if `C-u` and the count were typed: at most
    /// [`MAX_COUNT`] either way.
    pub count: Option<i16>,
}

/// A group of heads, drawn as one column under an optional title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    title: Option<String>,
    row: u8,
    heads: std::ops::Range<usize>,
}

impl Group {
    /// The title over its column, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The row of columns it is in: 1 to [`MAX_ROWS`].
    pub fn row(&self) -> u8 {
        self.row
    }

    /// Its heads, as indexes into [`Definition::heads`].
    pub fn heads(&self) -> std::ops::Range<usize> {
        self.heads.clone()
    }
}

/// What keys typed in an open menu come to, by [`Definition::lookup`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    /// They are this head's keys.
    Head(usize),
    /// They begin a head's keys: wait for the next.
    Pending,
    /// They are `q`, which no head binds: close the menu.
    Quit,
    /// They are no head's keys: a foreign key.
    Unbound,
}

/// Whether `name` is lower-case kebab case and short enough: `[a-z0-9]+(-[a-z0-9]+)*`, at most
/// [`MAX_NAME_BYTES`] bytes.
fn is_name(name: &str) -> bool {
    name.len() <= MAX_NAME_BYTES
        && !name.is_empty()
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Whether `key` builds a prefix argument in an open menu (ADR 0117, Q15): `C-u`, `M-<digit>`
/// or `M--`, so a head on it could never run. The same keys as the tip crate's prefix bindings
/// (`bindings/prefix.rs`), which must change with these.
fn builds_prefix(key: KeyPress) -> bool {
    match key.key {
        Key::Char('u') => key.modifiers == Modifiers::CONTROL,
        Key::Char(c) if c.is_ascii_digit() || c == '-' => key.modifiers == Modifiers::META,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
