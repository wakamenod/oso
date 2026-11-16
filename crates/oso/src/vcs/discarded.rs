//! The record of what a discard kept (ADR 0127, step 1c): one file in oso's state directory,
//! `vcs/discarded`, private, whose lines name each copy: when, the repository's root, the
//! object's id and the path, never the contents. The copies are git's own objects, unreachable,
//! which git keeps for `gc.pruneExpire`, two weeks by default.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; reading and writing it are the
//! threads' that run git.
//!
//! - A line is `TIME\tROOT\tID\tPATH`, the time in seconds since the Unix epoch and the root and
//!   path in hexadecimal, so that no name can make a line of its own. A line that does not
//!   parse is skipped.
//! - Entries older than [`KEEP`] go as the file is written, and past [`MAX_ENTRIES`] the oldest;
//!   at most [`MAX_FILE`] bytes of it are read.
//! - It is written whole to a file beside it, made private, then renamed over it, under a lock
//!   on a file beside it, as a status's thread and a run thread may write it at once; what is
//!   written never passes what is read back, the oldest lines going first.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long an entry is kept: as long as git keeps an unreachable object by default.
pub(crate) const KEEP: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// The most entries the record keeps.
pub(crate) const MAX_ENTRIES: usize = 10_000;
/// The most bytes of the record read.
pub(crate) const MAX_FILE: u64 = 4 * 1024 * 1024;

/// One copy a discard kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    /// When, in seconds since the Unix epoch.
    pub(crate) time: u64,
    /// The repository's root, as bytes of the path.
    pub(crate) root: Vec<u8>,
    /// The object's id, in hexadecimal.
    pub(crate) id: String,
    /// The path from the root, as git names it.
    pub(crate) path: Vec<u8>,
}

/// Seconds since the Unix epoch, now.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The record's file under `state`, oso's own state directory.
pub(crate) fn file(state: &Path) -> PathBuf {
    state.join("vcs").join("discarded")
}

/// The entries of `text`, oldest first; lines that do not parse are left out.
pub(crate) fn parse(text: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    for line in text.split(|&byte| byte == b'\n') {
        let mut fields = line.split(|&byte| byte == b'\t');
        let (Some(time), Some(root), Some(id), Some(path), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let time = std::str::from_utf8(time).ok().and_then(|t| t.parse().ok());
        let id = std::str::from_utf8(id).ok().filter(|id| is_id(id));
        let (Some(time), Some(root), Some(id), Some(path)) = (time, unhex(root), id, unhex(path))
        else {
            continue;
        };
        if root.is_empty() || path.is_empty() {
            continue;
        }
        out.push(Entry {
            time,
            root,
            id: id.to_owned(),
            path,
        });
    }
    out
}

/// The text of `entries`, one line each.
pub(crate) fn format(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        out.extend_from_slice(
            format!(
                "{}\t{}\t{}\t{}\n",
                entry.time,
                hex(&entry.root),
                entry.id,
                hex(&entry.path)
            )
            .as_bytes(),
        );
    }
    out
}

/// `entries` without those older than [`KEEP`] at `now`, and at most the newest
/// [`MAX_ENTRIES`], oldest first.
pub(crate) fn pruned(mut entries: Vec<Entry>, now: u64) -> Vec<Entry> {
    let oldest = now.saturating_sub(KEEP.as_secs());
    entries.retain(|entry| entry.time >= oldest && entry.time <= now.saturating_add(60));
    entries.sort_by_key(|entry| entry.time);
    let over = entries.len().saturating_sub(MAX_ENTRIES);
    entries.drain(..over);
    entries
}

/// The entries of the record under `state`: none if it does not exist.
pub(crate) fn read(state: &Path) -> io::Result<Vec<Entry>> {
    let mut text = Vec::new();
    match fs::File::open(file(state)) {
        Ok(opened) => {
            opened.take(MAX_FILE).read_to_end(&mut text)?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    }
    Ok(parse(&text))
}

/// Adds `added` to the record under `state`, pruned as of now, and writes it again: whole to a
/// private file beside it, then renamed over it.
pub(crate) fn add(state: &Path, added: Vec<Entry>) -> io::Result<()> {
    let path = file(state);
    let dir = path.parent().unwrap_or(state);
    fs::create_dir_all(dir)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(dir.join(".discarded.lock"))?;
    lock.lock()?;
    let mut entries = read(state)?;
    entries.extend(added);
    let entries = pruned(entries, now());
    let next = dir.join(".discarded.next");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut out = options.open(&next)?;
    // Its own, whatever a file left there before had.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        out.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    out.write_all(newest_within(&format(&entries)))?;
    out.sync_all()?;
    drop(out);
    let renamed = fs::rename(&next, &path);
    drop(lock);
    renamed
}

/// The end of `text` that fits in what is read back, from a line's start.
fn newest_within(text: &[u8]) -> &[u8] {
    let limit = usize::try_from(MAX_FILE).unwrap_or(usize::MAX);
    if text.len() <= limit {
        return text;
    }
    let from = text.len() - limit;
    let from = (text[from..].iter())
        .position(|&byte| byte == b'\n')
        .map_or(text.len(), |at| from + at + 1);
    &text[from..]
}

/// The entries of the repository at `root`, newest first.
pub(crate) fn of_root(entries: Vec<Entry>, root: &[u8]) -> Vec<Entry> {
    let mut mine: Vec<Entry> = entries.into_iter().filter(|e| e.root == root).collect();
    mine.reverse();
    mine
}

/// Whether `id` is an object id: 40 or 64 lower-case hexadecimal digits.
pub(crate) fn is_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    out
}

fn unhex(text: &[u8]) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    text.chunks(2)
        .map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{Entry, KEEP, MAX_ENTRIES, add, format, of_root, parse, pruned, read};

    fn entry(time: u64, path: &[u8]) -> Entry {
        Entry {
            time,
            root: b"/home/me/repo".to_vec(),
            id: "a".repeat(40),
            path: path.to_vec(),
        }
    }

    #[test]
    fn entries_go_and_come_back_whatever_their_names_hold() {
        let entries = vec![entry(1, b"src/a.rs"), entry(2, b"bad\nname\tx")];
        let text = format(&entries);
        assert_eq!(text.iter().filter(|&&b| b == b'\n').count(), 2);
        assert_eq!(parse(&text), entries);
        assert_eq!(parse(b"1\tzz\tabc\t00\nnot a line\n"), []);
    }

    #[test]
    fn old_entries_and_those_past_the_limit_go() {
        let now = KEEP.as_secs() + 100;
        let kept = pruned(vec![entry(now, b"new"), entry(50, b"old")], now);
        assert_eq!(kept, [entry(now, b"new")]);
        let many: Vec<Entry> = (0..MAX_ENTRIES as u64 + 3)
            .map(|n| entry(now - n, b"f"))
            .collect();
        let kept = pruned(many, now);
        assert_eq!(kept.len(), MAX_ENTRIES);
        assert_eq!(kept.last().unwrap().time, now);
    }

    #[test]
    fn the_record_is_written_private_and_read_back_by_root() {
        let dir = std::env::temp_dir().join(format!("oso-discarded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read(&dir).unwrap(), []);
        let now = super::now();
        add(&dir, vec![entry(now, b"a")]).unwrap();
        let mut other = entry(now, b"b");
        other.root = b"/elsewhere".to_vec();
        add(&dir, vec![other, entry(now, b"c")]).unwrap();
        let mine = of_root(read(&dir).unwrap(), b"/home/me/repo");
        let paths: Vec<&[u8]> = mine.iter().map(|e| e.path.as_slice()).collect();
        assert_eq!(paths, [&b"c"[..], &b"a"[..]]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(super::file(&dir))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    proptest! {
        #[test]
        fn any_bytes_parse_to_entries_that_format_and_parse_again(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
            let entries = parse(&bytes);
            prop_assert_eq!(parse(&format(&entries)), entries);
        }
    }
}
