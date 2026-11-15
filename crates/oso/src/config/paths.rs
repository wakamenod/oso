//! Paths the file names on this computer: an absolute one, or `~/` and a relative one, kept as
//! written for the app to join to the home directory when it uses them.
//!
//! Must not depend on: `app` or the file system; nothing is looked at on disk here.

use std::path::{Component, Path};

use toml::Spanned;
use toml::de::DeValue;

use super::problems::Found;
use super::type_name;

/// `label`'s path, as written: absolute or `~/` and a relative one, not on another computer,
/// of at most `max` bytes, with no NUL. `kind` says what it names in a complaint, such as
/// "a file's". A bad one is said and gives `None`.
pub(super) fn user_path(
    value: &Spanned<DeValue<'_>>,
    label: &str,
    kind: &str,
    max: usize,
    found: &mut Found,
) -> Option<String> {
    let at = value.span().start;
    let DeValue::String(path) = value.get_ref() else {
        found.at(
            at,
            format_args!(
                "{label} takes {kind} path, not {}",
                type_name(value.get_ref())
            ),
        );
        return None;
    };
    let absolute = path.starts_with("~/") || Path::new(path.as_ref()).is_absolute();
    // A share on another computer, or a Windows device, is no file of the person's here.
    let elsewhere = path.starts_with("\\\\") || path.starts_with("//");
    // Joined to the home directory, a rooted path would replace it.
    let rooted = path.strip_prefix("~/").is_some_and(|rest| {
        let rest = Path::new(rest);
        let prefix = matches!(rest.components().next(), Some(Component::Prefix(_)));
        rest.as_os_str().to_string_lossy().starts_with(['/', '\\']) || rest.has_root() || prefix
    });
    let why = if elsewhere {
        "takes a path on this computer, not one starting with \\\\ or //".to_owned()
    } else if !absolute {
        "takes an absolute path, or one starting with ~/".to_owned()
    } else if rooted {
        "takes ~/ and a relative path, not ~/ and one from a root".to_owned()
    } else if path.len() > max {
        format!("takes a path of at most {max} bytes")
    } else if path.contains('\0') {
        "takes a path with no NUL character".to_owned()
    } else {
        return Some(path.as_ref().to_owned());
    };
    found.at(at, format_args!("{label} {why}"));
    None
}

/// `written`, a path [`user_path`] took, with `~/` joined to `home`. `None` for `~/` with no
/// home directory or a relative one, and for a path that is then on another computer.
pub fn expand(written: &str, home: Option<&Path>) -> Option<std::path::PathBuf> {
    let path = match written.strip_prefix("~/") {
        Some(rest) => home.filter(|home| home.is_absolute())?.join(rest),
        None => std::path::PathBuf::from(written),
    };
    let text = path.as_os_str().to_string_lossy();
    let elsewhere = text.starts_with("//") || text.starts_with("\\\\");
    (path.is_absolute() && !elsewhere).then_some(path)
}
