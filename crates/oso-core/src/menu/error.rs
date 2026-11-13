//! Why a menu, a group or a head was refused (ADR 0117). Messages are sentences that quote no
//! buffer text: a configuration file's problem prefixes them with its line.
//!
//! Must not depend on: `keymap`.

use std::fmt;

use super::{
    MAX_COLUMNS, MAX_COUNT, MAX_HEADS, MAX_KEYS, MAX_LABEL_BYTES, MAX_LABEL_CELLS, MAX_MENUS,
    MAX_ROWS,
};

/// Why [`Definition`](super::Definition) or [`Menus`](super::Menus) refused something. Each one
/// leaves what it was given to unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuError {
    /// A menu's name is not lower-case kebab case of at most 64 bytes.
    BadName,
    /// A title is over 64 cells or 512 bytes.
    LongTitle,
    /// A label is over 64 cells or 512 bytes.
    LongLabel,
    /// A group's row is not 1 to 3.
    BadRow,
    /// A row already has 8 columns.
    TooManyColumns,
    /// The menu already has 128 heads.
    TooManyHeads,
    /// A head came before any group.
    NoGroup,
    /// A head has no keys.
    NoKeys,
    /// A head has more than 3 keys.
    TooManyKeys,
    /// A head's keys include `C-g`, which always cancels.
    Cancel,
    /// A head's keys include `C-u`, `M-<digit>` or `M--`, which build a prefix argument in a
    /// menu.
    ReservedKey,
    /// Another head of the menu has the same keys.
    KeyUsedTwice,
    /// A head's keys begin another head's, or another head's begin its own.
    KeyIsPrefix,
    /// A head's count is over 1000 either way.
    BadCount,
    /// A switch's name is not lower-case kebab case of at most 64 bytes.
    BadSwitchName,
    /// Another head of the menu flips the same switch.
    SwitchUsedTwice,
    /// No head has this index.
    NoSuchHead,
    /// The menu already has a default head.
    DefaultTwice,
    /// A head's keys begin with `RET` while another head is the one `RET` runs.
    RetReserved,
    /// The set already has 256 menus.
    TooManyMenus,
    /// A menu of this name is already defined.
    NameTaken,
    /// The definition's name is not the one its id was given for.
    WrongName,
}

impl fmt::Display for MenuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = "lower-case kebab case, such as \"windows\", of at most 64 bytes";
        match self {
            Self::BadName => write!(f, "a menu's name must be {names}"),
            Self::LongTitle => write!(
                f,
                "a title takes at most {MAX_LABEL_CELLS} cells and {MAX_LABEL_BYTES} bytes"
            ),
            Self::LongLabel => write!(
                f,
                "a label takes at most {MAX_LABEL_CELLS} cells and {MAX_LABEL_BYTES} bytes"
            ),
            Self::BadRow => write!(f, "a group's row is 1 to {MAX_ROWS}"),
            Self::TooManyColumns => write!(f, "a row has at most {MAX_COLUMNS} groups"),
            Self::TooManyHeads => write!(f, "a menu has at most {MAX_HEADS} heads"),
            Self::NoGroup => write!(f, "a head must be in a group"),
            Self::NoKeys => write!(f, "a head needs a key"),
            Self::TooManyKeys => write!(f, "a head takes at most {MAX_KEYS} keys"),
            Self::Cancel => write!(f, "C-g is not configurable; it always cancels"),
            Self::ReservedKey => write!(
                f,
                "C-u, M-<digit> and M-- build a prefix argument in a menu, so no head can use them"
            ),
            Self::KeyUsedTwice => write!(f, "another head of this menu has these keys"),
            Self::KeyIsPrefix => write!(f, "these keys begin another head's, or it begins these"),
            Self::BadCount => write!(f, "a count is at most {MAX_COUNT} either way"),
            Self::BadSwitchName => write!(f, "a switch's name must be {names}"),
            Self::SwitchUsedTwice => write!(f, "another head of this menu flips this switch"),
            Self::NoSuchHead => write!(f, "the menu has no such head"),
            Self::DefaultTwice => write!(f, "the menu already has a default head"),
            Self::RetReserved => write!(
                f,
                "RET runs the menu's default head, so no other head's keys may begin with it"
            ),
            Self::TooManyMenus => write!(f, "there are at most {MAX_MENUS} menus"),
            Self::NameTaken => write!(f, "a menu of this name is already defined"),
            Self::WrongName => write!(f, "the definition's name is not its id's"),
        }
    }
}

impl std::error::Error for MenuError {}
