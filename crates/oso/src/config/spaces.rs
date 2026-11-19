//! `[spaces]`: where the directories of issue spaces go (ADR 0136, Q1), and how a new space is
//! named (ADR 0159).
//!
//! Must not depend on: `app` or the file system; the app makes and checks the directories.

use toml::Spanned;
use toml::de::DeValue;

use super::paths::user_path;
use super::problems::{Found, cut, quoted};
use super::{table, type_name, unknown};

/// The longest `[spaces] root`, in bytes.
pub(super) const MAX_ROOT: usize = 1024;

/// What a space's directory goes under when `[spaces] root` says nothing.
pub const DEFAULT_ROOT: &str = "~/spaces";

/// How a new space is named (ADR 0159).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SpacesNaming {
    /// The person's `claude` summarizes the description, oso's own names standing in where
    /// it gives none.
    #[default]
    Claude,
    /// oso's own names: the description's first ASCII words, else the repository's name.
    Local,
}

/// `[spaces]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spaces {
    /// `root`: the directory each new space's own directory is made in, as written: an
    /// absolute path, or one starting with `~/`. Else [`DEFAULT_ROOT`].
    pub root: Option<String>,
    /// `naming`: `"claude"`, the default, or `"local"`.
    pub naming: SpacesNaming,
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
                let label = "[spaces] root";
                let root = user_path(value, label, "a directory's", MAX_ROOT, found);
                // A `..` would be refused when the directory is made; it is said here instead.
                let up = |root: &String| {
                    std::path::Path::new(root)
                        .components()
                        .any(|part| part == std::path::Component::ParentDir)
                };
                spaces.root = match root {
                    Some(root) if up(&root) => {
                        let at = value.span().start;
                        found.at(at, format_args!("{label} takes a path with no `..`"));
                        None
                    }
                    root => root,
                };
            }
            "naming" => match value.get_ref() {
                DeValue::String(text) if text == "claude" => spaces.naming = SpacesNaming::Claude,
                DeValue::String(text) if text == "local" => spaces.naming = SpacesNaming::Local,
                other => {
                    let what = match other {
                        DeValue::String(text) => quoted(cut(text.as_ref())).to_string(),
                        other => type_name(other).to_owned(),
                    };
                    found.at(
                        value.span().start,
                        format_args!("[spaces] naming takes \"claude\" or \"local\", not {what}"),
                    );
                    // Written to keep descriptions from a model, most likely: fail closed.
                    spaces.naming = SpacesNaming::Local;
                }
            },
            other => unknown(found, name.span().start, other, "[spaces]"),
        }
    }
    spaces
}
