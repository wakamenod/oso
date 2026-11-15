//! `[git]`: what git's runs with the repository's hooks are given, `pass_env`, the names of
//! variables passed on from oso's own environment (ADR 0127, step 1b).
//!
//! Must not depend on: `app` or the file system; nothing is looked at here.

use toml::Spanned;
use toml::de::DeValue;

use super::problems::Found;
use super::terminal::{env, refused};
use super::{table, unknown};

/// `[git]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSettings {
    /// `pass_env`: names of variables passed on from oso's own environment to a run of git
    /// whose hooks run, beside the process module's allowlist and the SSH agent's and GnuPG's.
    pub pass_env: Option<Vec<String>>,
}

/// `[git]`.
pub(super) fn git(value: &Spanned<DeValue<'_>>, found: &mut Found) -> GitSettings {
    let mut git = GitSettings::default();
    let Some(entries) = table(value, "[git]", found) else {
        return git;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "pass_env" => git.pass_env = env(value, "[git] pass_env", git_refused, found),
            other => unknown(found, name.span().start, other, "[git]"),
        }
    }
    git
}

/// Why `name` cannot be passed on to git's hooks, if it cannot: as [`refused`] says, and git's
/// own variables are oso's to set, as one could send git to another repository. On Windows,
/// whose names ignore case, so does this.
fn git_refused(name: &str) -> Option<&'static str> {
    let upper = name.to_ascii_uppercase();
    let own = if cfg!(windows) { upper.as_str() } else { name };
    if own.starts_with("GIT_") {
        Some("git's own variables are oso's to set")
    } else {
        refused(name)
    }
}
