//! `[terminal]`: the shell a terminal starts, whether as a login shell, the environment names
//! it is given, and how much scrollback it keeps (ADR 0120, Q11 and Q18).
//!
//! Must not depend on: `app`, `dispatch` or the file system; nothing is looked at on disk
//! here. Whether the shell exists is found when a terminal starts, off the UI thread.

use std::path::PathBuf;

use toml::Spanned;
use toml::de::DeValue;

use super::problems::{Found, cut, quoted};
use super::settings::switch;
use super::{table, type_name, unknown};

/// The most scrollback lines (ADR 0120, Q11).
pub(super) const MAX_SCROLLBACK: usize = 100_000;
/// The most names `[terminal] env` passes on.
pub(super) const MAX_ENV_NAMES: usize = 64;
/// The longest name `[terminal] env` takes, in bytes.
pub(super) const MAX_ENV_NAME: usize = 256;
/// The longest `[terminal] shell` path, in bytes.
pub(super) const MAX_SHELL_PATH: usize = 4096;

/// Names a terminal's environment sets itself, which `[terminal] env` cannot pass on.
const OWN_NAMES: &[&str] = &[
    "SHELL",
    "TERM",
    "COLORTERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "OSO_TERMINAL_TOKEN",
];

/// `[terminal]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Terminal {
    /// `shell`: an absolute path to the shell a terminal starts.
    pub shell: Option<PathBuf>,
    /// `login`: whether the shell starts as a login shell (`-l`).
    pub login: Option<bool>,
    /// `env`: names of variables passed on from oso's own environment.
    pub env: Option<Vec<String>>,
    /// `scrollback`: the lines a terminal keeps above its screen.
    pub scrollback: Option<usize>,
}

/// `[terminal]`.
pub(super) fn terminal(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Terminal {
    let mut terminal = Terminal::default();
    let Some(entries) = table(value, "[terminal]", found) else {
        return terminal;
    };
    for (name, value) in entries {
        match name.get_ref().as_ref() {
            "shell" => terminal.shell = shell(value, found),
            "login" => terminal.login = switch(value, "[terminal]", "login", found),
            "env" => terminal.env = env(value, found),
            "scrollback" => terminal.scrollback = scrollback(value, found),
            other => unknown(found, name.span().start, other, "[terminal]"),
        }
    }
    terminal
}

/// `[terminal] shell`: an absolute path of at most [`MAX_SHELL_PATH`] bytes, with no NUL.
fn shell(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<PathBuf> {
    let at = value.span().start;
    let DeValue::String(path) = value.get_ref() else {
        found.at(
            at,
            format_args!(
                "[terminal] shell takes an executable's absolute path, not {}",
                type_name(value.get_ref())
            ),
        );
        return None;
    };
    let why = if !std::path::Path::new(path.as_ref()).is_absolute() {
        "takes an absolute path; the next shell is used"
    } else if path.len() > MAX_SHELL_PATH {
        "takes a path of at most 4096 bytes"
    } else if path.contains('\0') {
        "takes a path with no NUL character"
    } else {
        return Some(PathBuf::from(path.as_ref()));
    };
    found.at(at, format_args!("[terminal] shell {why}"));
    None
}

/// `[terminal] env`: a list of names, each refused that the process module would refuse or
/// that a terminal sets itself. The rest are kept.
fn env(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<Vec<String>> {
    let at = value.span().start;
    let DeValue::Array(items) = value.get_ref() else {
        found.at(
            at,
            format_args!(
                "[terminal] env takes a list of variables' names, not {}",
                type_name(value.get_ref())
            ),
        );
        return None;
    };
    let mut names = Vec::new();
    for item in items {
        let at = item.span().start;
        let DeValue::String(name) = item.get_ref() else {
            found.at(
                at,
                format_args!(
                    "[terminal] env takes names, not {}",
                    type_name(item.get_ref())
                ),
            );
            continue;
        };
        if let Some(why) = refused(name) {
            found.at(
                at,
                format_args!("[terminal] env {}: {why}, skipped", quoted(cut(name))),
            );
        } else if names.len() == MAX_ENV_NAMES {
            found.at(
                at,
                format_args!(
                    "[terminal] env takes at most {MAX_ENV_NAMES} names, the rest skipped"
                ),
            );
            break;
        } else if !names.iter().any(|kept| kept == name.as_ref()) {
            names.push(name.as_ref().to_owned());
        }
    }
    Some(names)
}

/// Why `name` cannot be passed on, if it cannot: the process module refuses an empty name, one
/// holding `=` or NUL, `PATH` (its own) and the loader's `LD_*` and `DYLD_*` (ADR 0120, step 2),
/// and a terminal sets some names itself.
fn refused(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        Some("an empty name")
    } else if name.len() > MAX_ENV_NAME {
        Some("a name of at most 256 bytes")
    } else if name.contains(['=', '\0']) || name.chars().any(char::is_control) {
        Some("a name holds no =, NUL or control character")
    } else if name == "PATH" {
        Some("PATH is oso's own, filtered")
    } else if name.starts_with("LD_") || name.starts_with("DYLD_") {
        Some("the loader's variables are never passed on")
    } else if OWN_NAMES.contains(&name) {
        Some("a terminal sets it itself")
    } else {
        None
    }
}

/// `[terminal] scrollback`: a whole number of lines from 0 to [`MAX_SCROLLBACK`].
fn scrollback(value: &Spanned<DeValue<'_>>, found: &mut Found) -> Option<usize> {
    let refuse = |found: &mut Found, what: &dyn std::fmt::Display| {
        found.at(
            value.span().start,
            format_args!(
                "[terminal] scrollback takes a number of lines from 0 to {MAX_SCROLLBACK}, not {what}"
            ),
        );
        None
    };
    let DeValue::Integer(n) = value.get_ref() else {
        return refuse(found, &type_name(value.get_ref()));
    };
    match i64::from_str_radix(n.as_str(), n.radix())
        .ok()
        .and_then(|lines| usize::try_from(lines).ok())
    {
        Some(lines) if lines <= MAX_SCROLLBACK => Some(lines),
        _ => refuse(found, &cut(n)),
    }
}
