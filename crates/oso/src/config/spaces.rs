//! `[spaces]`: where the directories of issue spaces go (ADR 0136, Q1).
//!
//! Must not depend on: `app` or the file system; the app makes and checks the directories.

use toml::Spanned;
use toml::de::DeValue;

use super::paths::user_path;
use super::problems::Found;
use super::{table, unknown};

/// The longest `[spaces] root`, in bytes.
pub(super) const MAX_ROOT: usize = 1024;

/// What a space's directory goes under when `[spaces] root` says nothing.
pub const DEFAULT_ROOT: &str = "~/spaces";

/// `[spaces]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spaces {
    /// `root`: the directory each new space's own directory is made in, as written: an
    /// absolute path, or one starting with `~/`. Else [`DEFAULT_ROOT`].
    pub root: Option<String>,
}

impl Spaces {
    /// The root in force, as written.
    pub fn root(&self) -> &str {
        self.root.as_deref().unwrap_or(DEFAULT_ROOT)
    }
}

/// `[spaces]`.
pub(super) fn spaces(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Spaces {
    let mut spaces = Spaces::default();
    let Some(entries) = table(value, "[spaces]", found) else {
        return spaces;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "root" => {
                spaces.root = user_path(value, "[spaces] root", "a directory's", MAX_ROOT, found);
            }
            other => unknown(found, name.span().start, other, "[spaces]"),
        }
    }
    spaces
}
