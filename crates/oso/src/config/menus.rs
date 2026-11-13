//! The menus from the file's `[menus]` table (ADR 0117): each `[menus.<name>]` adds a menu, or
//! replaces a built-in one of that name whole, and `[menus.<name>] = false` removes a built-in
//! one. Each head that breaks a rule is skipped with its line, and the rest of its menu loads.
//! A removed built-in menu's default key is left undefined in the user keymap.
//!
//! Must not depend on: `app` or `dispatch`; it builds a [`Menus`] for the dispatcher to take,
//! from the registry and built-in menus it is given. Nothing in a menu runs: a head names a
//! registered command, a switch, another menu, or nothing.

use std::borrow::Cow;
use std::collections::HashMap;

use oso_core::command::Registry;
use oso_core::key::{KeyPress, KeySequence};
use oso_core::keymap::Keymap;
use oso_core::menu::{
    Definition, ForeignKeys, Head, HeadAction, MAX_HEADS, MAX_KEYS, MenuError, MenuId, Menus, Stay,
};
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::problems::{Found, quoted};
use super::{TableName, table, type_name, unknown};

/// What a head's entry is, for a message about one that is not.
const HEAD_SHAPE: &str = "a head is [keys, action, label], with an optional table of stay, \
                          count and default";

/// One `[menus.<name>]` table of the file, with the id it defines.
struct Table<'t, 'i> {
    id: MenuId,
    name: &'t str,
    table: &'t DeTable<'i>,
}

/// The menus in force: `builtin`, with what the `[menus]` table `value` adds, replaces and
/// removes. Entries with a problem are skipped, with a message in `found`.
pub(super) fn menus(
    value: &Spanned<DeValue<'_>>,
    registry: &Registry,
    builtin: &Menus,
    found: &mut Found,
) -> Menus {
    let mut menus = builtin.clone();
    let Some(entries) = table(value, "[menus]", found) else {
        return menus;
    };
    // Removals first, and every name the file defines gets its id, so that a head can name a
    // menu defined after it, whatever the order of the tables.
    let mut tables = Vec::new();
    for (name, value) in entries {
        let at = name.span().start;
        let name: &str = name.get_ref();
        match value.get_ref() {
            DeValue::Boolean(false) => match builtin.lookup(name) {
                Some(id) => menus.remove(id),
                None => found.about(
                    at,
                    name,
                    format_args!("no built-in menu has this name, so none is removed"),
                ),
            },
            DeValue::Table(table) => match menus.id(name) {
                Ok(id) => tables.push(Table { id, name, table }),
                Err(error) => found.about(at, name, format_args!("{error}; skipped")),
            },
            other => found.about(
                at,
                name,
                format_args!(
                    "a menu is a table, or false to remove a built-in one, not {}",
                    type_name(other)
                ),
            ),
        }
    }
    // By name, so that a head naming a menu costs one look, however many tables there are.
    let by_name: HashMap<&str, MenuId> = tables.iter().map(|t| (t.name, t.id)).collect();
    let defined = |name: &str| by_name.get(name).copied().or_else(|| menus.lookup(name));
    let mut built = Vec::with_capacity(tables.len());
    for table in &tables {
        let mut read = Read {
            registry,
            defined: &defined,
            found: &mut *found,
        };
        if let Some(definition) = read.definition(table) {
            built.push((table.id, definition));
        }
    }
    for (id, definition) in built {
        // A built-in menu of this name is replaced whole.
        menus.remove(id);
        let _ = menus.define(id, definition);
    }
    menus
}

/// What reading one menu needs.
struct Read<'r, 'f, D> {
    registry: &'r Registry,
    /// The id of the menu of this name that the file leaves defined, if one is.
    defined: &'r D,
    found: &'f mut Found,
}

impl<D: Fn(&str) -> Option<MenuId>> Read<'_, '_, D> {
    /// The menu `table` defines, with every entry that has a problem left out. Its name is its
    /// title until a title is given. `None` only for a name the id has checked already.
    fn definition(&mut self, table: &Table<'_, '_>) -> Option<Definition> {
        let mut definition = Definition::new(table.name, table.name).ok()?;
        let mut groups = None;
        for (name, value) in table.table {
            let at = name.span().start;
            match name.get_ref().as_ref() {
                "title" => {
                    if let Some(title) = self.string(value, "a title") {
                        match Definition::new(table.name, title) {
                            Ok(titled) => definition = titled,
                            Err(error) => {
                                self.found.at(value.span().start, format_args!("{error}"))
                            }
                        }
                    }
                }
                "groups" => groups = Some(value),
                "foreign-keys" | "default-head" => {}
                other => unknown(
                    self.found,
                    at,
                    other,
                    &format!("[menus.{}]", TableName(table.name)),
                ),
            }
        }
        // The rules and the groups after the title, which makes the definition anew.
        for (name, value) in table.table {
            match name.get_ref().as_ref() {
                "foreign-keys" => {
                    if let Some(rule) = self.choice(value, "foreign-keys", &["warn", "exit", "run"])
                    {
                        definition.set_foreign_keys(match rule {
                            "exit" => ForeignKeys::Exit,
                            "run" => ForeignKeys::Run,
                            _ => ForeignKeys::Warn,
                        });
                    }
                }
                "default-head" => {
                    if let Some(rule) = self.choice(value, "default-head", &["exit", "stay"]) {
                        definition.set_default_stay(if rule == "stay" {
                            Stay::Stay
                        } else {
                            Stay::Exit
                        });
                    }
                }
                _ => {}
            }
        }
        if let Some(groups) = groups {
            self.groups(&mut definition, table, groups);
        }
        Some(definition)
    }

    /// The groups of `[[menus.<name>.groups]]`, `value`, into `definition`.
    fn groups(
        &mut self,
        definition: &mut Definition,
        table: &Table<'_, '_>,
        value: &Spanned<DeValue<'_>>,
    ) {
        let DeValue::Array(groups) = value.get_ref() else {
            self.found.at(
                value.span().start,
                format_args!(
                    "groups is a list of tables, [[menus.{}.groups]]",
                    TableName(table.name)
                ),
            );
            return;
        };
        for group in groups {
            let Some(entries) = super::table(group, "a group", self.found) else {
                continue;
            };
            let (mut title, mut row, mut heads) = (None, 1, None);
            for (name, value) in entries {
                match name.get_ref().as_ref() {
                    "title" => title = self.string(value, "a group's title"),
                    // A row past a `u8` is as wrong as 4, which the definition refuses.
                    "row" => {
                        row = integer(value)
                            .and_then(|n| u8::try_from(n).ok())
                            .unwrap_or(0);
                    }
                    "heads" => heads = Some(value),
                    other => unknown(self.found, name.span().start, other, "a group"),
                }
            }
            if let Err(error) = definition.add_group(title, row) {
                self.found.at(
                    group.span().start,
                    format_args!("{error}; the group is skipped"),
                );
                continue;
            }
            if heads.is_some_and(|heads| self.heads(definition, table, heads)) {
                return;
            }
        }
    }

    /// The heads of a group, `value`, into `definition`'s last group. True if the menu is full
    /// and the rest of its heads were skipped.
    fn heads(
        &mut self,
        definition: &mut Definition,
        table: &Table<'_, '_>,
        value: &Spanned<DeValue<'_>>,
    ) -> bool {
        let DeValue::Array(heads) = value.get_ref() else {
            self.found
                .at(value.span().start, format_args!("heads is a list of heads"));
            return false;
        };
        for head in heads {
            let at = head.span().start;
            // The rest of the menu's heads are not read: the definition would refuse each.
            if definition.heads().len() == MAX_HEADS {
                self.found.at(
                    at,
                    format_args!(
                        "{}; this head and the rest are skipped",
                        MenuError::TooManyHeads
                    ),
                );
                return true;
            }
            let Some((spelled, head, default)) = self.head(table, head) else {
                continue;
            };
            match definition.add_head(head) {
                Ok(index) if default => {
                    if let Err(error) = definition.set_ret_head(index) {
                        self.found.about(
                            at,
                            spelled,
                            format_args!("{error}; it is not made the default head"),
                        );
                    }
                }
                Ok(_) => {}
                Err(error) => self
                    .found
                    .about(at, spelled, format_args!("{error}; skipped")),
            }
        }
        false
    }

    /// One head, with its keys as the file spells them and whether it is the default head.
    fn head<'v>(
        &mut self,
        table: &Table<'_, '_>,
        value: &'v Spanned<DeValue<'_>>,
    ) -> Option<(&'v str, Head, bool)> {
        let at = value.span().start;
        let parts = match value.get_ref() {
            DeValue::Array(parts) if (3..=4).contains(&parts.len()) => parts,
            _ => {
                self.found.at(at, format_args!("{HEAD_SHAPE}"));
                return None;
            }
        };
        let DeValue::String(spelled) = parts[0].get_ref() else {
            self.found
                .at(at, format_args!("{HEAD_SHAPE}; its keys are a string"));
            return None;
        };
        let spelled: &str = spelled;
        let keys = self.keys(at, spelled)?;
        let action = self.action(table, spelled, &parts[1])?;
        let DeValue::String(label) = parts[2].get_ref() else {
            self.found.about(
                at,
                spelled,
                format_args!("{HEAD_SHAPE}; its label is a string"),
            );
            return None;
        };
        let mut head = Head {
            keys,
            label: label.to_string(),
            action,
            stay: None,
            count: None,
        };
        let mut default = false;
        if let Some(options) = parts.get(3) {
            default = self.options(spelled, options, &mut head)?;
        }
        Some((spelled, head, default))
    }

    /// The keys `spelled`, if they parse: at most one more than a head takes, which the
    /// definition then refuses.
    fn keys(&mut self, at: usize, spelled: &str) -> Option<KeySequence> {
        let mut keys = Vec::new();
        for token in spelled.split_ascii_whitespace().take(MAX_KEYS + 1) {
            let Ok(key) = token.parse::<KeyPress>() else {
                self.found.about(
                    at,
                    spelled,
                    format_args!(
                        "{} is not a key; write keys as Emacs does, such as C-x, M-<f5> or SPC",
                        quoted(token)
                    ),
                );
                return None;
            };
            keys.push(key);
        }
        Some(KeySequence(keys))
    }

    /// What a head does: a command's name, `{ menu = … }`, `{ switch = … }` or `false`.
    fn action(
        &mut self,
        table: &Table<'_, '_>,
        spelled: &str,
        value: &Spanned<DeValue<'_>>,
    ) -> Option<HeadAction> {
        let at = value.span().start;
        match value.get_ref() {
            DeValue::Boolean(false) => return Some(HeadAction::Quit),
            DeValue::String(name) => match self.registry.lookup(name) {
                Some(command) => return Some(HeadAction::Command(command)),
                None => self.found.about(
                    at,
                    spelled,
                    format_args!("no command is named {}", quoted(name)),
                ),
            },
            DeValue::Table(entries) if entries.len() == 1 => {
                let (kind, name) = entries.iter().next()?;
                let kind_is = |k: &str| kind.get_ref().as_ref() == k;
                let DeValue::String(name) = name.get_ref() else {
                    // Only a known kind is named in the message: the key is the file's text.
                    match ["menu", "switch"].into_iter().find(|k| kind_is(k)) {
                        Some(known) => self.found.about(
                            at,
                            spelled,
                            format_args!("a {known} is named with a string"),
                        ),
                        None => self.found.about(at, spelled, format_args!("{ACTIONS}")),
                    }
                    return None;
                };
                match kind.get_ref().as_ref() {
                    "menu" if name.as_ref() == table.name => {
                        self.found
                            .about(at, spelled, format_args!("a menu cannot open itself"))
                    }
                    "menu" => match (self.defined)(name) {
                        Some(menu) => return Some(HeadAction::Menu(menu)),
                        None => self.found.about(
                            at,
                            spelled,
                            format_args!("no menu is named {}", quoted(name)),
                        ),
                    },
                    "switch" => return Some(HeadAction::Switch(name.to_string())),
                    _ => self.found.about(at, spelled, format_args!("{}", ACTIONS)),
                }
            }
            _ => self.found.about(at, spelled, format_args!("{}", ACTIONS)),
        }
        None
    }

    /// A head's table of options into `head`; whether it is the default head. `None` if one is
    /// wrong, which skips the head.
    fn options(
        &mut self,
        spelled: &str,
        value: &Spanned<DeValue<'_>>,
        head: &mut Head,
    ) -> Option<bool> {
        let DeValue::Table(options) = value.get_ref() else {
            self.found
                .about(value.span().start, spelled, format_args!("{HEAD_SHAPE}"));
            return None;
        };
        let mut default = false;
        let (no_stay, no_count) = (ignores_stay(&head.action), ignores_count(&head.action));
        for (name, value) in options {
            let at = value.span().start;
            match (name.get_ref().as_ref(), value.get_ref(), no_stay, no_count) {
                ("stay", DeValue::Boolean(_), Some(what), _) | ("count", _, _, Some(what)) => {
                    self.found.about(
                        at,
                        spelled,
                        format_args!("{} does nothing on {what}; ignored", name.get_ref()),
                    );
                }
                ("stay", DeValue::Boolean(stay), ..) => {
                    head.stay = Some(if *stay { Stay::Stay } else { Stay::Exit });
                }
                ("default", DeValue::Boolean(is), ..) => default = *is,
                ("count", ..) => match integer(value).and_then(|n| i16::try_from(n).ok()) {
                    Some(count) => head.count = Some(count),
                    None => {
                        self.found.about(
                            at,
                            spelled,
                            format_args!("a count is a whole number from -1000 to 1000"),
                        );
                        return None;
                    }
                },
                ("stay" | "default", other, ..) => {
                    self.found.about(
                        at,
                        spelled,
                        format_args!(
                            "{} is true or false, not {}",
                            name.get_ref(),
                            type_name(other)
                        ),
                    );
                    return None;
                }
                (other, ..) => unknown(self.found, name.span().start, other, "a head's options"),
            }
        }
        Some(default)
    }

    /// The string `value`, called `what` in a message if it is something else.
    fn string<'v>(&mut self, value: &'v Spanned<DeValue<'_>>, what: &str) -> Option<&'v str> {
        if let DeValue::String(text) = value.get_ref() {
            return Some(text);
        }
        self.found.at(
            value.span().start,
            format_args!("{what} is a string, not {}", type_name(value.get_ref())),
        );
        None
    }

    /// One of `choices`, the string `value`, called `name` in a message if it is another.
    fn choice<'v>(
        &mut self,
        value: &'v Spanned<DeValue<'_>>,
        name: &str,
        choices: &[&str],
    ) -> Option<&'v str> {
        let given = match value.get_ref() {
            DeValue::String(text) => Some(Cow::as_ref(text)),
            _ => None,
        };
        if let Some(given) = given.filter(|given| choices.contains(given)) {
            return Some(given);
        }
        self.found.at(
            value.span().start,
            format_args!("{name} is one of {}", Choices(choices)),
        );
        None
    }
}

/// What a head whose `stay` does nothing is, for a message: one that always stays or always exits
/// (`Definition::stay`).
fn ignores_stay(action: &HeadAction) -> Option<&'static str> {
    match action {
        HeadAction::Command(_) => None,
        HeadAction::Switch(_) => Some("a switch, which always stays"),
        HeadAction::Menu(_) => Some("a head that opens a menu, which always stays"),
        HeadAction::Quit => Some("a head that only closes the menu"),
    }
}

/// What a head whose `count` does nothing is, for a message: one that runs no command. A head
/// that opens a menu passes its count on.
fn ignores_count(action: &HeadAction) -> Option<&'static str> {
    match action {
        HeadAction::Command(_) | HeadAction::Menu(_) => None,
        HeadAction::Switch(_) => Some("a switch"),
        HeadAction::Quit => Some("a head that only closes the menu"),
    }
}

/// What a head's action may be, for a message about one that is something else.
const ACTIONS: &str = "a head's action is a command's name, { menu = \"name\" }, \
                       { switch = \"name\" }, or false to only close the menu";

/// The whole number `value`, if it is one that fits.
fn integer(value: &Spanned<DeValue<'_>>) -> Option<i64> {
    let DeValue::Integer(n) = value.get_ref() else {
        return None;
    };
    i64::from_str_radix(&n.as_str().replace('_', ""), n.radix()).ok()
}

/// Choices, quoted, for a message: `"warn", "exit" or "run"`.
struct Choices<'c>(&'c [&'c str]);

impl std::fmt::Display for Choices<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, choice) in self.0.iter().enumerate() {
            match i {
                0 => {}
                _ if i + 1 == self.0.len() => f.write_str(" or ")?,
                _ => f.write_str(", ")?,
            }
            write!(f, "{choice:?}")?;
        }
        Ok(())
    }
}

/// The default keys bound to a built-in menu `menus` no longer has.
pub(super) fn removed_menu_keys(menus: &Menus, defaults: &Keymap) -> Vec<Vec<KeyPress>> {
    let mut keys = Vec::new();
    defaults.menu_bindings(|sequence, id| {
        if menus.get(id).is_none() {
            keys.push(sequence.to_vec());
        }
    });
    keys
}

/// Unbinds, in the user's keymap `user`, each default key whose built-in menu `menus` no longer
/// has, so that the key is undefined as any unbound key is, and `open-menu` and the key list do
/// not show it (ADR 0117, step 1's Q7). A key the file binds or unbinds itself is the file's.
pub(super) fn unbind_removed_menus(user: &mut Keymap, menus: &Menus, defaults: &Keymap) {
    for keys in removed_menu_keys(menus, defaults) {
        if user.lookup(&keys).is_none() {
            // Fails only under a key the file binds to a command or a menu, which the default
            // key cannot reach then anyway.
            let _ = user.unbind(&keys);
        }
    }
}
