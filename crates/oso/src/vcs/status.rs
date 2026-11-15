//! What `git status --porcelain=v2 -z --branch` and `git diff --numstat -z` say, read into
//! entries (ADR 0127).
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; these are pure functions of a
//! child's output, which is untrusted: a record that does not parse is skipped, never trusted
//! in part, and nothing here panics or grows past its input.
//!
//! Paths stay bytes as git gives them with `-z`, `/` on every OS, so that a name that is not
//! UTF-8 still names its file.

use std::collections::HashMap;

/// The branch headers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Branch {
    /// The commit `HEAD` is on, as hex, or `None` before the first commit.
    pub(crate) oid: Option<String>,
    /// The branch checked out, or `None` when `HEAD` is detached.
    pub(crate) head: Option<Vec<u8>>,
    pub(crate) upstream: Option<Vec<u8>>,
    /// Commits ahead of the upstream and behind it, when git counted them.
    pub(crate) ahead_behind: Option<(u64, u64)>,
}

/// What kind of record an entry came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Changed in the index, the work tree or both.
    Changed,
    /// Renamed or copied in the index, from `from`.
    Moved {
        from: Vec<u8>,
        copied: bool,
    },
    /// Unmerged: its letters are the pair, such as `UU` or `AA`.
    Unmerged,
    Untracked,
}

/// One path's status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) path: Vec<u8>,
    pub(crate) kind: Kind,
    /// The index's letter against `HEAD`, `.` when unchanged (`X`).
    pub(crate) staged: u8,
    /// The work tree's letter against the index, `.` when unchanged (`Y`).
    pub(crate) unstaged: u8,
    pub(crate) submodule: bool,
}

/// A whole status.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) branch: Branch,
    pub(crate) entries: Vec<Entry>,
}

/// Reads the output of `git status --porcelain=v2 -z --branch`. A record that does not parse
/// is skipped; a header that does not parse is left unset.
pub(crate) fn parse(bytes: &[u8]) -> Status {
    let mut status = Status::default();
    let mut records = bytes.split(|&byte| byte == 0);
    while let Some(record) = records.next() {
        match record.first() {
            Some(b'#') => header(&mut status.branch, record),
            Some(b'1') => {
                if let Some(entry) = ordinary(record) {
                    status.entries.push(entry);
                }
            }
            Some(b'2') => {
                // The path it came from is the next record.
                let from = records.next();
                if let Some(entry) = from.and_then(|from| moved(record, from)) {
                    status.entries.push(entry);
                }
            }
            Some(b'u') => {
                if let Some(entry) = unmerged(record) {
                    status.entries.push(entry);
                }
            }
            Some(b'?') => {
                if let Some(path) = record.strip_prefix(b"? ").filter(|path| !path.is_empty()) {
                    status.entries.push(Entry {
                        path: path.to_vec(),
                        kind: Kind::Untracked,
                        staged: b'?',
                        unstaged: b'?',
                        submodule: false,
                    });
                }
            }
            _ => {}
        }
    }
    status
}

fn header(branch: &mut Branch, record: &[u8]) {
    let Some(rest) = record.strip_prefix(b"# ") else {
        return;
    };
    let (name, value) = split_once(rest, b' ').unwrap_or((rest, b""));
    match name {
        b"branch.oid" => {
            branch.oid = (value != b"(initial)" && is_hex(value))
                .then(|| String::from_utf8_lossy(value).into_owned());
        }
        b"branch.head" => {
            branch.head = (value != b"(detached)" && !value.is_empty()).then(|| value.to_vec());
        }
        b"branch.upstream" if !value.is_empty() => branch.upstream = Some(value.to_vec()),
        b"branch.ab" => {
            let mut counts = value.split(|&byte| byte == b' ');
            let ahead = counts.next().and_then(|n| n.strip_prefix(b"+"));
            let behind = counts.next().and_then(|n| n.strip_prefix(b"-"));
            branch.ahead_behind = number(ahead).zip(number(behind));
        }
        _ => {}
    }
}

/// `1 XY SUB MH MI MW HH HI PATH`.
fn ordinary(record: &[u8]) -> Option<Entry> {
    let fields = fields(record, 9)?;
    let (staged, unstaged) = letters(fields[1])?;
    let path = fields[8];
    (!path.is_empty()).then(|| Entry {
        path: path.to_vec(),
        kind: Kind::Changed,
        staged,
        unstaged,
        submodule: fields[2].first() == Some(&b'S'),
    })
}

/// `2 XY SUB MH MI MW HH HI XSCORE PATH`, then the path it came from.
fn moved(record: &[u8], from: &[u8]) -> Option<Entry> {
    let fields = fields(record, 10)?;
    let (staged, unstaged) = letters(fields[1])?;
    let path = fields[9];
    let copied = fields[8].first() == Some(&b'C');
    (!path.is_empty() && !from.is_empty()).then(|| Entry {
        path: path.to_vec(),
        kind: Kind::Moved {
            from: from.to_vec(),
            copied,
        },
        staged,
        unstaged,
        submodule: fields[2].first() == Some(&b'S'),
    })
}

/// `u XY SUB M1 M2 M3 MW H1 H2 H3 PATH`.
fn unmerged(record: &[u8]) -> Option<Entry> {
    let fields = fields(record, 11)?;
    let (staged, unstaged) = letters(fields[1])?;
    let path = fields[10];
    (!path.is_empty()).then(|| Entry {
        path: path.to_vec(),
        kind: Kind::Unmerged,
        staged,
        unstaged,
        submodule: fields[2].first() == Some(&b'S'),
    })
}

/// The first `n - 1` fields split at single spaces, then the rest, a path that may hold
/// spaces.
fn fields(record: &[u8], n: usize) -> Option<Vec<&[u8]>> {
    let fields: Vec<&[u8]> = record.splitn(n, |&byte| byte == b' ').collect();
    (fields.len() == n).then_some(fields)
}

/// The two status letters, each a printable ASCII byte.
fn letters(xy: &[u8]) -> Option<(u8, u8)> {
    match xy {
        [x, y] if x.is_ascii_graphic() && y.is_ascii_graphic() => Some((*x, *y)),
        _ => None,
    }
}

fn number(digits: Option<&[u8]>) -> Option<u64> {
    let digits = digits?;
    if digits.is_empty() || digits.len() > 19 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

fn is_hex(value: &[u8]) -> bool {
    !value.is_empty() && value.len() <= 64 && value.iter().all(u8::is_ascii_hexdigit)
}

fn split_once(bytes: &[u8], at: u8) -> Option<(&[u8], &[u8])> {
    let index = bytes.iter().position(|&byte| byte == at)?;
    Some((&bytes[..index], &bytes[index + 1..]))
}

/// A file's lines added and removed, or `None` for a binary file.
pub(crate) type Counts = Option<(u64, u64)>;

/// Reads the output of `git diff --numstat -z`: each path's counts, by its path, or by its new
/// path for a rename. A record that does not parse is skipped.
pub(crate) fn parse_numstat(bytes: &[u8]) -> HashMap<Vec<u8>, Counts> {
    let mut counts = HashMap::new();
    let mut records = bytes.split(|&byte| byte == 0);
    while let Some(record) = records.next() {
        let mut parts = record.splitn(3, |&byte| byte == b'\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            // A rename: the old path, then the new one, each a record of its own.
            let _from = records.next();
            match records.next() {
                Some(to) if !to.is_empty() => to,
                _ => continue,
            }
        } else {
            path
        };
        let value = match (added, removed) {
            (b"-", b"-") => None,
            _ => match number(Some(added)).zip(number(Some(removed))) {
                Some(pair) => Some(pair),
                None => continue,
            },
        };
        counts.insert(path.to_vec(), value);
    }
    counts
}

#[cfg(test)]
mod tests;
