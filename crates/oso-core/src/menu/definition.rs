//! A menu's definition, checked as it is built, so that the built-in tables and `config.toml`
//! meet the same rules and limits (ADR 0117), and the lookup of keys typed in it.
//!
//! Must not depend on: `keymap`, which binds keys to the menus defined here.

use std::fmt;

use super::{
    Describe, ForeignKeys, Group, Head, HeadAction, Lookup, MAX_COLUMNS, MAX_COUNT, MAX_HEADS,
    MAX_KEYS, MAX_LABEL_CELLS, MAX_ROWS, MenuError, Stay, builds_prefix, is_cancel, is_name,
};
use crate::key::{Key, KeyPress};
use crate::minibuffer::cells_of;

/// `q`, which closes a menu where no head binds it.
const QUIT: KeyPress = KeyPress::plain(Key::Char('q'));

/// A menu: its name, title, rules, groups and heads.
#[derive(Clone)]
pub struct Definition {
    name: String,
    title: String,
    foreign_keys: ForeignKeys,
    default_head: Stay,
    default: Option<usize>,
    describe: Option<Describe>,
    groups: Vec<Group>,
    heads: Vec<Head>,
}

impl Definition {
    /// An empty menu named `name`, which must be lower-case kebab case of at most 64 bytes,
    /// under `title`, of at most 64 cells. Its heads exit and it refuses foreign keys until set
    /// otherwise.
    pub fn new(name: &str, title: &str) -> Result<Self, MenuError> {
        if !is_name(name) {
            return Err(MenuError::BadName);
        }
        if cells_of(title) > MAX_LABEL_CELLS {
            return Err(MenuError::LongTitle);
        }
        Ok(Self {
            name: name.to_owned(),
            title: title.to_owned(),
            foreign_keys: ForeignKeys::default(),
            default_head: Stay::default(),
            default: None,
            describe: None,
            groups: Vec::new(),
            heads: Vec::new(),
        })
    }

    /// What a key it does not bind does.
    pub fn set_foreign_keys(&mut self, rule: ForeignKeys) {
        self.foreign_keys = rule;
    }

    /// What a head that says nothing does: exit, or stay.
    pub fn set_default_head(&mut self, stay: Stay) {
        self.default_head = stay;
    }

    /// The function that gives notes as it is drawn, for a built-in menu.
    pub fn set_describe(&mut self, describe: Describe) {
        self.describe = Some(describe);
    }

    /// Starts a group, in row `row` (1 to 3), with an optional title of at most 64 cells. The
    /// heads added next go in it.
    pub fn add_group(&mut self, title: Option<&str>, row: u8) -> Result<(), MenuError> {
        if !(1..=MAX_ROWS).contains(&row) {
            return Err(MenuError::BadRow);
        }
        if title.is_some_and(|title| cells_of(title) > MAX_LABEL_CELLS) {
            return Err(MenuError::LongTitle);
        }
        if self.groups.iter().filter(|group| group.row == row).count() == MAX_COLUMNS {
            return Err(MenuError::TooManyColumns);
        }
        let start = self.heads.len();
        self.groups.push(Group {
            title: title.map(str::to_owned),
            row,
            heads: start..start,
        });
        Ok(())
    }

    /// Adds `head` to the last group, and returns its index. Refused if it breaks a limit, if
    /// its keys include `C-g`, `C-u`, `M-<digit>` or `M--`, if another head has the same keys or
    /// keys one begins the other's, or if another head flips the same switch.
    pub fn add_head(&mut self, head: Head) -> Result<usize, MenuError> {
        if self.groups.is_empty() {
            return Err(MenuError::NoGroup);
        }
        if self.heads.len() == MAX_HEADS {
            return Err(MenuError::TooManyHeads);
        }
        self.check(&head)?;
        let index = self.heads.len();
        self.heads.push(head);
        if let Some(group) = self.groups.last_mut() {
            group.heads.end = index + 1;
        }
        Ok(index)
    }

    /// Whether `head` may join the heads there are.
    fn check(&self, head: &Head) -> Result<(), MenuError> {
        let keys = &head.keys.0;
        if keys.is_empty() {
            return Err(MenuError::NoKeys);
        }
        if keys.len() > MAX_KEYS {
            return Err(MenuError::TooManyKeys);
        }
        if keys.iter().copied().any(is_cancel) {
            return Err(MenuError::Cancel);
        }
        if keys.iter().copied().any(builds_prefix) {
            return Err(MenuError::ReservedKey);
        }
        if cells_of(&head.label) > MAX_LABEL_CELLS {
            return Err(MenuError::LongLabel);
        }
        if head
            .count
            .is_some_and(|count| count.unsigned_abs() > MAX_COUNT.unsigned_abs())
        {
            return Err(MenuError::BadCount);
        }
        for other in &self.heads {
            let theirs = &other.keys.0;
            if theirs == keys {
                return Err(MenuError::KeyUsedTwice);
            }
            if theirs.starts_with(keys) || keys.starts_with(theirs) {
                return Err(MenuError::KeyIsPrefix);
            }
        }
        if let HeadAction::Switch(name) = &head.action {
            if !is_name(name) {
                return Err(MenuError::BadSwitchName);
            }
            let same = |other: &Head| matches!(&other.action, HeadAction::Switch(n) if n == name);
            if self.heads.iter().any(same) {
                return Err(MenuError::SwitchUsedTwice);
            }
        }
        Ok(())
    }

    /// Makes `head` the one `RET` runs. A describe function may name another as it draws.
    pub fn set_default(&mut self, head: usize) -> Result<(), MenuError> {
        if head >= self.heads.len() {
            return Err(MenuError::NoSuchHead);
        }
        if self.default.is_some() {
            return Err(MenuError::DefaultTwice);
        }
        self.default = Some(head);
        Ok(())
    }

    /// Its name, which names it in `config.toml`, a key's binding and `open-menu`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The title over its columns.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// What a key it does not bind does.
    pub fn foreign_keys(&self) -> ForeignKeys {
        self.foreign_keys
    }

    /// What a head that says nothing does.
    pub fn default_head(&self) -> Stay {
        self.default_head
    }

    /// The head `RET` runs, unless a describe function names another.
    pub fn default(&self) -> Option<usize> {
        self.default
    }

    /// The function that gives notes as it is drawn, for a built-in menu that has one.
    pub fn describe(&self) -> Option<Describe> {
        self.describe
    }

    /// Its groups, in order.
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Its heads, group after group: a head is named by its index here.
    pub fn heads(&self) -> &[Head] {
        &self.heads
    }

    /// Whether the menu stays after `head`: its own rule, or the menu's default. A switch always
    /// stays, and so does a head that opens another menu, which is pushed over this one (Q16); a
    /// head that only closes the menu always exits. Exit for an index it has no head at.
    pub fn stay(&self, head: usize) -> Stay {
        let Some(head) = self.heads.get(head) else {
            return Stay::Exit;
        };
        match head.action {
            HeadAction::Switch(_) | HeadAction::Menu(_) => Stay::Stay,
            HeadAction::Quit => Stay::Exit,
            HeadAction::Command(_) => head.stay.unwrap_or(self.default_head),
        }
    }

    /// What `keys`, typed in the open menu, come to: a head, the start of one, `q` to quit, or
    /// nothing. A pass over at most 128 heads of at most 3 keys.
    pub fn lookup(&self, keys: &[KeyPress]) -> Lookup {
        if keys.is_empty() {
            return Lookup::Unbound;
        }
        let mut pending = false;
        for (index, head) in self.heads.iter().enumerate() {
            let theirs = &head.keys.0[..];
            if theirs == keys {
                return Lookup::Head(index);
            }
            pending |= theirs.starts_with(keys);
        }
        if pending {
            Lookup::Pending
        } else if keys == [QUIT] && self.quits_on_q() {
            Lookup::Quit
        } else {
            Lookup::Unbound
        }
    }

    /// Whether `q` closes the menu: no head's keys begin with it.
    pub fn quits_on_q(&self) -> bool {
        !self
            .heads
            .iter()
            .any(|head| head.keys.0.first() == Some(&QUIT))
    }
}

impl fmt::Debug for Definition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Definition")
            .field("name", &self.name)
            .field("title", &self.title)
            .field("foreign_keys", &self.foreign_keys)
            .field("default_head", &self.default_head)
            .field("default", &self.default)
            .field("describe", &self.describe.is_some())
            .field("groups", &self.groups)
            .field("heads", &self.heads)
            .finish()
    }
}
