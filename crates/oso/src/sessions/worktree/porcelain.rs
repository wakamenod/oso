//! What git prints, parsed (ADR 0121, "Worktree sessions"): the `-z` outputs of `git status`,
//! `git ls-files`, `git for-each-ref` and `git log -g`, read as raw bytes. `git worktree list`'s
//! is the `git` module's (`git::worktrees`), shared with a review (ADR 0122, step 12b).
//! Git's output is untrusted: a path or a branch name may hold any byte, so nothing here
//! panics, every list has a bound, and [`printable`] makes text safe to show a person.
//!
//! Must not depend on: processes, the file system, `app` or `dispatch`; these are
//! plain functions over bytes the worker read.

use std::path::PathBuf;

use crate::git::{os_lossy, printable};

/// The most characters [`status_lines`] shows of one entry.
const MAX_LINE_CHARS: usize = 1024;

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
        .map(os_lossy)
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

#[cfg(test)]
mod tests;
