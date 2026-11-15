//! `file:` URIs to paths and back (ADR 0138). A URI a server sends is read only if it names
//! a local file by an absolute path in normal form: scheme `file`, an empty host or
//! `localhost`, no query or fragment, and segments that are names, never `.`, `..`, empty or
//! holding a separator once decoded. So an answer cannot name another computer through its
//! host, or through a UNC path on Windows, which would send the person's credentials over SMB.
//!
//! Must not depend on: `framing`, `json` or `position`; this sees only text and paths.
//!
//! On Windows a URI carries its drive letter (`file:///C:/…`), read without case and with
//! `%3A` as `:`, and written in capitals. A segment that Windows would read as something
//! other than a plain name is refused: one with `:` (a stream), `\`, a trailing dot or space,
//! or a device's name such as `NUL` or `COM1`. The rules for both systems are plain
//! functions of text, so each system's tests check the other's too.
//!
//! These are rules of text, not of the file the path reaches: a path that the system maps to
//! another computer (macOS's `/net` automount, a mapped drive), a device file on Unix, or a
//! Windows 8.3 short name passes them. What opens a path from a server checks the file itself
//! (ADR 0138, step 2 as built).

use std::path::{Path, PathBuf};

use crate::limits::URI_BYTES_MAX;

/// The `file:` URI of `path`, or `None` if it is not absolute and in normal form, has a
/// segment a URI from a server could not name, or would be over [`URI_BYTES_MAX`] (see the
/// module docs).
pub fn from_path(path: &Path) -> Option<String> {
    let (drive, segments) = os::segments(path)?;
    let windows = cfg!(windows);
    if segments
        .iter()
        .any(|segment| !valid_segment(segment, windows))
    {
        return None;
    }
    Some(encode(drive, &segments)).filter(|uri| uri.len() <= URI_BYTES_MAX)
}

/// The path that `uri` names, or `None` if it is not a `file:` URI of a local, absolute path
/// in normal form (see the module docs).
pub fn to_path(uri: &str) -> Option<PathBuf> {
    let parts = parse(uri, cfg!(windows))?;
    os::path(&parts)
}

/// What a `file:` URI names: a drive letter, in capitals, on Windows, and the decoded
/// segments after it.
#[derive(Debug, PartialEq, Eq)]
struct Parts {
    drive: Option<u8>,
    segments: Vec<Vec<u8>>,
}

/// Bytes a URI's path keeps as they are (RFC 3986's unreserved ones); every other byte of
/// a segment is written `%XX`.
fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn encode(drive: Option<u8>, segments: &[Vec<u8>]) -> String {
    let mut uri = String::from("file://");
    if let Some(letter) = drive {
        uri.push('/');
        uri.push(char::from(letter));
        uri.push(':');
    }
    if segments.is_empty() {
        uri.push('/');
    }
    for segment in segments {
        uri.push('/');
        for &byte in segment {
            if unreserved(byte) {
                uri.push(char::from(byte));
            } else {
                uri.push('%');
                uri.push(hex_digit(byte >> 4));
                uri.push(hex_digit(byte & 0xF));
            }
        }
    }
    uri
}

fn hex_digit(nibble: u8) -> char {
    char::from(b"0123456789ABCDEF"[usize::from(nibble & 0xF)])
}

/// Reads `uri` by the rules of Windows (`windows`) or of Unix. One over [`URI_BYTES_MAX`] is
/// refused before anything is decoded.
fn parse(uri: &str, windows: bool) -> Option<Parts> {
    if uri.len() > URI_BYTES_MAX {
        return None;
    }
    let rest = strip_prefix_ignore_case(uri, "file://")?;
    let slash = rest.find('/')?;
    let host = &rest[..slash];
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    let mut segments = rest[slash + 1..].split('/').peekable();
    let drive = if windows {
        Some(drive(&decode(segments.next()?)?)?)
    } else {
        None
    };
    let mut decoded = Vec::new();
    while let Some(segment) = segments.next() {
        // A trailing `/`, as a directory's URI may have, names nothing more.
        if segment.is_empty() && segments.peek().is_none() {
            break;
        }
        let segment = decode(segment)?;
        if !valid_segment(&segment, windows) {
            return None;
        }
        decoded.push(segment);
    }
    Some(Parts {
        drive,
        segments: decoded,
    })
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// A segment's bytes with its `%XX` decoded, or `None` for a bad escape, an ASCII control
/// written as it is, or a query or fragment, which no file's URI has.
fn decode(segment: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(segment.len());
    let mut rest = segment.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        match byte {
            b'%' => {
                let (high, low) = (rest.first()?, rest.get(1)?);
                let value = hex_value(*high)? << 4 | hex_value(*low)?;
                bytes.push(value);
                rest = &rest[2..];
            }
            b'?' | b'#' => return None,
            byte if byte.is_ascii_control() => return None,
            byte => bytes.push(byte),
        }
    }
    Some(bytes)
}

fn hex_value(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

/// The drive letter in capitals of a decoded first segment `C:`, or `None`.
fn drive(segment: &[u8]) -> Option<u8> {
    match segment {
        [letter, b':'] if letter.is_ascii_alphabetic() => Some(letter.to_ascii_uppercase()),
        _ => None,
    }
}

/// Whether a decoded segment is a plain name, by the rules of Windows or of Unix.
fn valid_segment(segment: &[u8], windows: bool) -> bool {
    if segment.is_empty() || segment == b"." || segment == b".." || segment.contains(&0) {
        return false;
    }
    if segment.contains(&b'/') {
        return false;
    }
    if !windows {
        return true;
    }
    let Ok(name) = std::str::from_utf8(segment) else {
        return false;
    };
    !(name.contains(['\\', ':', '<', '>', '"', '|', '?', '*'])
        || name.ends_with(['.', ' '])
        || name.chars().any(char::is_control)
        || device_name(name))
}

/// Whether Windows reads `name` as a device, whatever its extension: `CON`, `PRN`, `AUX`,
/// `NUL`, `COM0`–`COM9` and `LPT0`–`LPT9`, with superscript digits too, without case.
fn device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&upper.as_str()) {
        return true;
    }
    let mut chars = upper.chars();
    let head: String = chars.by_ref().take(3).collect();
    let digit = chars.next();
    (head == "COM" || head == "LPT")
        && chars.next().is_none()
        && digit.is_some_and(|digit| digit.is_ascii_digit() || "¹²³".contains(digit))
}

#[cfg(not(windows))]
mod os {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Component, Path, PathBuf};

    use super::Parts;

    /// The segments of an absolute path in normal form.
    pub(super) fn segments(path: &Path) -> Option<(Option<u8>, Vec<Vec<u8>>)> {
        let mut components = path.components();
        if components.next() != Some(Component::RootDir) {
            return None;
        }
        let segments = components
            .map(|component| match component {
                Component::Normal(name) => Some(name.as_bytes().to_vec()),
                _ => None,
            })
            .collect::<Option<_>>()?;
        Some((None, segments))
    }

    pub(super) fn path(parts: &Parts) -> Option<PathBuf> {
        let mut path = PathBuf::from("/");
        for segment in &parts.segments {
            path.push(OsStr::from_bytes(segment));
        }
        Some(path)
    }
}

#[cfg(windows)]
mod os {
    use std::path::{Component, Path, PathBuf, Prefix};

    use super::Parts;

    /// The drive letter and segments of an absolute path on a drive, also in its verbatim
    /// form (`\\?\C:\`); never a UNC or a device's path.
    pub(super) fn segments(path: &Path) -> Option<(Option<u8>, Vec<Vec<u8>>)> {
        let mut components = path.components();
        let letter = match components.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter)
                    if letter.is_ascii_alphabetic() =>
                {
                    letter.to_ascii_uppercase()
                }
                _ => return None,
            },
            _ => return None,
        };
        if components.next() != Some(Component::RootDir) {
            return None;
        }
        let segments = components
            .map(|component| match component {
                Component::Normal(name) => name.to_str().map(|name| name.as_bytes().to_vec()),
                _ => None,
            })
            .collect::<Option<_>>()?;
        Some((Some(letter), segments))
    }

    pub(super) fn path(parts: &Parts) -> Option<PathBuf> {
        let letter = char::from(parts.drive?);
        let mut path = PathBuf::from(format!("{letter}:\\"));
        for segment in &parts.segments {
            path.push(std::str::from_utf8(segment).ok()?);
        }
        Some(path)
    }
}

#[cfg(test)]
mod tests;
