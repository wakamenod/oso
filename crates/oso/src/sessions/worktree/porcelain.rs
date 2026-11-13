//! What git prints, parsed (ADR 0121, "Worktree sessions"): the `-z` outputs of `git worktree
//! list`, `git status`, `git ls-files`, `git for-each-ref` and `git log -g`, read as raw bytes.
//! Git's output is untrusted: a path or a branch name may hold any byte, so nothing here
//! panics, every list has a bound, and [`printable`] makes text safe to show a person.
//!
//! Must not depend on: processes, the file system, `app`, `dispatch` or `commands`; these are
//! plain functions over bytes the worker read.

use std::path::PathBuf;

/// The most characters [`status_lines`] shows of one entry.
const MAX_LINE_CHARS: usize = 1024;

/// One worktree as `git worktree list --porcelain -z` gives it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    /// Where it is.
    pub path: PathBuf,
    /// The commit checked out, when git named one in UTF-8.
    pub head: Option<String>,
    /// The branch's short name, without `refs/heads/`, when it is UTF-8.
    pub branch: Option<String>,
    /// The main worktree of a bare repository.
    pub bare: bool,
    /// No branch is checked out.
    pub detached: bool,
    /// `git worktree lock` holds it.
    pub locked: bool,
    /// Git would prune it: its directory is gone.
    pub prunable: bool,
}

/// The worktrees of `git worktree list --porcelain -z`: fields each ended by NUL, a record ended
/// by an empty field. Fields are `worktree <path>`, `HEAD <oid>`, `branch refs/heads/<name>`,
/// `bare`, `detached`, `locked[ <reason>]` and `prunable[ <reason>]`; others are ignored. A
/// record without `worktree` is dropped. At most `max` records are kept.
pub fn worktrees(bytes: &[u8], max: usize) -> Vec<Listed> {
    let mut listed = Vec::new();
    let mut record: Option<Listed> = None;
    for field in ended(bytes) {
        if listed.len() >= max {
            break;
        }
        if field.is_empty() {
            listed.extend(record.take());
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            // A record git did not end goes in as it is.
            let next = Listed {
                path: path_of(path),
                ..Listed::default()
            };
            listed.extend(record.replace(next));
        } else if let Some(record) = record.as_mut() {
            read_field(record, field);
        }
    }
    listed.extend(record);
    listed.truncate(max);
    listed
}

/// The NUL-ended fields of `bytes`: what follows the last NUL is no field, as when output was
/// cut short.
fn ended(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let end = bytes.iter().rposition(|&b| b == 0);
    end.map(|end| &bytes[..end])
        .into_iter()
        .flat_map(|fields| fields.split(|&b| b == 0))
}

/// Sets what `field` says on `record`.
fn read_field(record: &mut Listed, field: &[u8]) {
    let flag = |name: &[u8]| {
        field == name || (field.starts_with(name) && field.get(name.len()) == Some(&b' '))
    };
    if let Some(oid) = field.strip_prefix(b"HEAD ") {
        record.head = std::str::from_utf8(oid).ok().map(str::to_owned);
    } else if let Some(name) = field.strip_prefix(b"branch ") {
        let name = name.strip_prefix(b"refs/heads/").unwrap_or(name);
        record.branch = std::str::from_utf8(name)
            .ok()
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
    } else if field == b"bare" {
        record.bare = true;
    } else if field == b"detached" {
        record.detached = true;
    } else if flag(b"locked") {
        record.locked = true;
    } else if flag(b"prunable") {
        record.prunable = true;
    }
}

/// Counts from `git status --porcelain -z --untracked-files=all`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Status {
    /// Entries with a change git tracks: staged, unstaged or in conflict.
    pub changed: usize,
    /// Files git does not track and does not ignore.
    pub untracked: usize,
}

/// One entry of `git status --porcelain -z`.
struct Entry<'a> {
    code: [u8; 2],
    path: &'a [u8],
    /// The old path of a rename or a copy.
    from: Option<&'a [u8]>,
}

/// The entries of `git status --porcelain -z`: each `XY <path>` ended by NUL, and, when X or Y
/// is `R` or `C`, the old path in one more NUL-ended field. An entry too short to have a code
/// is skipped.
fn entries(bytes: &[u8]) -> impl Iterator<Item = Entry<'_>> {
    let mut fields = bytes.split(|&b| b == 0);
    std::iter::from_fn(move || {
        loop {
            let field = fields.next()?;
            let (Some(&x), Some(&y), Some(b' ')) = (field.first(), field.get(1), field.get(2))
            else {
                continue;
            };
            let from = if [x, y].iter().any(|&c| c == b'R' || c == b'C') {
                fields.next()
            } else {
                None
            };
            return Some(Entry {
                code: [x, y],
                path: &field[3..],
                from,
            });
        }
    })
}

/// The counts of `git status --porcelain -z --untracked-files=all`: `??` is untracked, `!!`
/// ignored and not counted, anything else changed.
pub fn status(bytes: &[u8]) -> Status {
    let mut status = Status::default();
    for entry in entries(bytes) {
        match &entry.code {
            b"??" => status.untracked += 1,
            b"!!" => {}
            _ => status.changed += 1,
        }
    }
    status
}

/// The entries of `git status --porcelain -z` as text for a person, `XY path`, or `XY old -> new`
/// for a rename or a copy, each made [`printable`]. At most `max`.
pub fn status_lines(bytes: &[u8], max: usize) -> Vec<String> {
    entries(bytes)
        .take(max)
        .map(|entry| {
            let code = String::from_utf8_lossy(&entry.code);
            let path = String::from_utf8_lossy(entry.path);
            let line = match entry.from {
                Some(from) => format!("{code} {} -> {path}", String::from_utf8_lossy(from)),
                None => format!("{code} {path}"),
            };
            printable(&line, MAX_LINE_CHARS)
        })
        .collect()
}

/// The paths of `git ls-files -z`, as given (a directory ends with `/`); empty fields are
/// dropped. At most `max`.
pub fn paths(bytes: &[u8], max: usize) -> Vec<PathBuf> {
    bytes
        .split(|&b| b == 0)
        .filter(|field| !field.is_empty())
        .take(max)
        .map(path_of)
        .collect()
}

/// NUL-ended names, as `git for-each-ref --format=%(refname:short)%00` prints them: the line
/// break git puts after each format is taken off the next field. Empty fields and names that
/// are not UTF-8 are dropped. At most `max`.
pub fn names(bytes: &[u8], max: usize) -> Vec<String> {
    bytes
        .split(|&b| b == 0)
        .map(|field| field.strip_prefix(b"\n").unwrap_or(field))
        .filter(|field| !field.is_empty())
        .filter_map(|field| std::str::from_utf8(field).ok())
        .take(max)
        .map(str::to_owned)
        .collect()
}

/// The pairs of `git for-each-ref --format=%(refname)%00%(symref)%00 refs/remotes/`, one per
/// line: the ref's full name and, for a symbolic ref, its target. A ref name never holds a line
/// break or a control character, so lines are split on `\n`. Lines without a name or not in
/// UTF-8 are dropped. At most `max`.
pub fn ref_pairs(bytes: &[u8], max: usize) -> Vec<(String, Option<String>)> {
    bytes
        .split(|&b| b == b'\n')
        .filter_map(|line| {
            let mut fields = line.split(|&b| b == 0);
            let name = std::str::from_utf8(fields.next()?).ok()?;
            if name.is_empty() {
                return None;
            }
            let target = std::str::from_utf8(fields.next().unwrap_or_default()).ok()?;
            let target = (!target.is_empty()).then(|| target.to_owned());
            Some((name.to_owned(), target))
        })
        .take(max)
        .collect()
}

/// Whether `git log -g --format=%gs -z refs/heads/B` shows a commit made on the branch: an
/// entry whose subject starts with `commit` (`commit:`, `commit (amend):`, `commit (merge):`,
/// `commit (initial):`), `cherry-pick` or `revert`.
pub fn made_a_commit(reflog: &[u8]) -> bool {
    reflog
        .split(|&b| b == 0)
        .map(|entry| entry.strip_prefix(b"\n").unwrap_or(entry))
        .any(|subject| {
            [b"commit".as_slice(), b"cherry-pick", b"revert"]
                .iter()
                .any(|start| subject.starts_with(start))
        })
}

/// Text safe to show a person: control characters, the line and paragraph separators and the
/// bidirectional controls become `\u{…}` escapes. At most `max` characters of it, then `…`.
pub fn printable(text: &str, max: usize) -> String {
    use std::fmt::Write;
    let mut shown = String::with_capacity(text.len().min(max.saturating_mul(4)));
    let mut count = 0;
    for c in text.chars() {
        let code = u32::from(c);
        // `\u{`, the hex digits, `}`; or the character itself.
        let width = if hidden(c) {
            4 + (32 - code.leading_zeros()).div_ceil(4).max(1) as usize
        } else {
            1
        };
        if count + width > max {
            shown.push('…');
            return shown;
        }
        count += width;
        if width == 1 {
            shown.push(c);
        } else {
            let _ = write!(shown, "\\u{{{code:x}}}");
        }
    }
    shown
}

/// Whether `c` would change how the text around it looks, or would not show: C0, DEL and C1
/// controls, U+2028 and U+2029, and the bidirectional marks, embeddings, overrides and isolates.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

/// A path from git's raw bytes: as they are on Unix, decoded as UTF-8 elsewhere.
fn path_of(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[cfg(test)]
mod tests;
