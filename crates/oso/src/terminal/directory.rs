//! The directory a terminal's program says it is in, as OSC 7's `file:` URI or OSC 9;9's or
//! OSC 1337 `CurrentDir`'s plain path, read as a local path or not at all (ADR 0141). It moves
//! only where a prompt for a path starts in that terminal, which the person sees before `RET`;
//! where a process starts never follows it.
//!
//! Must not depend on: `app`, `config` or `dispatch`, or the file system: nothing is looked at
//! here, and the prompt's listing says whether the directory is there.

use std::path::{Component, Path, PathBuf, Prefix};

use oso_core::display_map::cell::is_invisible;

/// The local directory `sent` names, sent by a terminal's program and already free of control
/// characters and at most 4 KiB (`oso-term`'s policy), on the machine named `host`: a `file:`
/// URI whose host is empty, `localhost` or this machine's, or a plain path. Either must be
/// absolute, on a local disk (no `//server/share`, UNC or `\\?\` path, which listing would
/// reach over the network), with no `.` or `..` in it, and a URI's `%` escapes must decode to
/// UTF-8; neither may hold a control character or one drawn as a box (bidi controls,
/// zero-width ones). `None` for anything else, such as a path on another machine after `ssh`.
pub(crate) fn local(sent: &str, host: Option<&str>) -> Option<PathBuf> {
    let path = match strip_scheme(sent) {
        Some(rest) => {
            let rest = rest.strip_prefix("//")?;
            let (named, path) = rest.split_at(rest.find('/')?);
            if !this_machine(named, host) {
                return None;
            }
            decoded(path)?
        }
        None => sent.to_owned(),
    };
    let path = drive_path(path);
    // Read by its words, as `Path` drops a `.` between two names.
    let plain = path
        .split(['/', std::path::MAIN_SEPARATOR])
        .all(|part| part != "." && part != "..");
    let shown = !path.chars().any(|c| c.is_control() || is_invisible(c));
    let path = PathBuf::from(path);
    (path.is_absolute() && plain && shown && on_a_local_disk(&path)).then_some(path)
}

/// Whether `path` starts at a local root: `/` alone on Unix, not `//`, and a drive letter on
/// Windows, not a server's share or a verbatim or device path.
fn on_a_local_disk(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy();
    if text.starts_with("//") || text.starts_with(r"\\") {
        return false;
    }
    match path.components().next() {
        Some(Component::Prefix(prefix)) => matches!(prefix.kind(), Prefix::Disk(_)),
        Some(Component::RootDir) => cfg!(unix),
        _ => false,
    }
}

/// What follows `file:`, whatever its case.
fn strip_scheme(sent: &str) -> Option<&str> {
    let scheme = sent.get(..5)?;
    scheme.eq_ignore_ascii_case("file:").then(|| &sent[5..])
}

/// Whether the URI's host `named` is this machine: empty, `localhost`, or `host`, without
/// case, or `host`'s first label alone (`mac` for `mac.local`), as a shell's `$HOST` may be.
/// A longer name than `host`'s, such as `mac.example` for `mac`, is another machine's.
fn this_machine(named: &str, host: Option<&str>) -> bool {
    if named.is_empty() || named.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let Some(host) = host else {
        return false;
    };
    let first = host.split('.').next().unwrap_or(host);
    named.eq_ignore_ascii_case(host) || named.eq_ignore_ascii_case(first)
}

/// `path` with its `%XX` escapes decoded, if they are whole and give UTF-8 with no control
/// character.
fn decoded(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    let text = String::from_utf8(out).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}

/// On Windows, `/C:/work` as a URI's path gives it is `C:/work`; elsewhere it is unchanged.
fn drive_path(path: String) -> String {
    if cfg!(windows) {
        let bytes = path.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b':'
        {
            return path[1..].to_owned();
        }
    }
    path
}

#[cfg(test)]
mod tests;
