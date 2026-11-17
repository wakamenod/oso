//! Staging a review's hunks (ADR 0122, "Decisions", step 5): accept stages hunks of the file
//! as the review shows them, in the staged scope reject unstages them, and undo applies the same
//! patch the other way. The patch is built from the hunks and the old side the review read, with
//! up to [`CONTEXT`] lines of it around each, so that git places it by its context in an index
//! that holds other staged hunks, and refuses it where the file moved on.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; it runs on the review's thread, which
//! reads the old side through its `git cat-file --batch`.
//!
//! - Each hunk's removed lines are checked to be the old side's lines where the hunk says; else
//!   nothing runs: "that hunk changed; look again".
//! - Hunks whose context meets go in one fragment, the old side's lines between them, as a hunk
//!   not staged leaves them, as context.
//! - A path git would quote is quoted as git writes it, so that no path can end the header.
//! - The patch goes to `git apply --cached` on standard input, whitespace compared exactly, as
//!   the git status stages one (ADR 0127).

use std::ffi::OsStr;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use oso_core::review::Hunk;

use super::batch::{Batch, Blob};
use crate::git::{Git, Id};

/// How many of the old side's lines go around each hunk.
pub(crate) const CONTEXT: usize = 3;

/// What says a hunk is not where the review read it.
pub(crate) const CHANGED: &str = "that hunk changed; look again";

/// What a stage applies.
pub(crate) enum Apply {
    /// Hunks of the file, built into a patch with context from the old side, the blob `old`.
    Hunks { old: Id, hunks: Vec<Hunk> },
    /// A patch built and applied before, as undo applies it the other way.
    Patch(Arc<[u8]>),
}

/// Hunks to put in the index, or to take out of it with `reverse`.
pub(crate) struct Stage {
    /// Which stage the host asked for, which its answer names.
    pub(crate) serial: u64,
    pub(crate) path: Box<[u8]>,
    pub(crate) apply: Apply,
    pub(crate) reverse: bool,
}

/// What a stage did: the patch it applied, or why not, with no file's text.
pub(crate) type Staged = Result<Arc<[u8]>, String>;

/// Builds `stage`'s patch, reading its old side through `batch`, and applies it to the index.
pub(super) fn run(git: &Git, batch: &mut Batch, stage: Stage) -> Staged {
    let patch = match stage.apply {
        Apply::Patch(patch) => patch,
        Apply::Hunks { old, hunks } => {
            let old = match batch.read(git, &old) {
                Ok(Blob::Bytes(bytes)) => bytes,
                Ok(_) => return Err("its old side could not be read".to_owned()),
                Err(e) => return Err(e.to_string()),
            };
            Arc::from(patch(&stage.path, &old, &hunks)?)
        }
    };
    let mut words = vec![
        "-c",
        "apply.ignoreWhitespace=no",
        "apply",
        "--cached",
        "--whitespace=nowarn",
    ];
    if stage.reverse {
        words.push("-R");
    }
    words.push("-");
    let args: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    let stop = AtomicBool::new(false);
    let out = git
        .run_long(&args, &[], Some(patch.to_vec()), &stop)
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(patch);
    }
    let first = out.stderr.lines().next().unwrap_or_default().trim();
    Err(format!(
        "git refused the hunk, as the file changed since it was read: {first}"
    ))
}

/// The old side's lines, each with its line break, the last maybe without one.
fn lines(text: &[u8]) -> Vec<&[u8]> {
    text.split_inclusive(|byte| *byte == b'\n').collect()
}

/// Where hunk `hunk`'s removed lines are in the old side: the index of its first, and of the one
/// after its last. A hunk that removes nothing goes in front of old line `old_start`.
fn span(hunk: &Hunk) -> (usize, usize) {
    let from = (hunk.old_start as usize).saturating_sub(1);
    (from, from + hunk.removed.len())
}

/// The patch of `hunks` of the file at `path`, whose old side is `old`: its header, then a
/// fragment for each run of hunks whose context meets.
///
/// # Errors
///
/// [`CHANGED`] when a hunk's removed lines are not the old side's where it says.
pub(crate) fn patch(path: &[u8], old: &[u8], hunks: &[Hunk]) -> Result<Vec<u8>, String> {
    let old = lines(old);
    let mut hunks: Vec<&Hunk> = hunks.iter().collect();
    hunks.sort_by_key(|hunk| hunk.old_start);
    for hunk in &hunks {
        let (from, to) = span(hunk);
        let same = old.get(from..to).is_some_and(|there| {
            there.len() == hunk.removed.len()
                && there.iter().zip(&hunk.removed).all(|(a, b)| **a == **b)
        });
        if !same {
            return Err(CHANGED.to_owned());
        }
    }
    let mut out = Vec::new();
    for (prefix, line) in [
        (&b"diff --git "[..], None),
        (b"--- ", Some(b"a/")),
        (b"+++ ", Some(b"b/")),
    ] {
        out.extend_from_slice(prefix);
        match line {
            Some(side) => quoted(side, path, &mut out),
            None => {
                quoted(b"a/", path, &mut out);
                out.push(b' ');
                quoted(b"b/", path, &mut out);
            }
        }
        out.push(b'\n');
    }
    let mut shift: i64 = 0;
    let mut at = 0;
    while at < hunks.len() {
        // The hunks of one fragment: each next one's context meets the last one's.
        let mut until = at + 1;
        while until < hunks.len() && span(hunks[until]).0 <= span(hunks[until - 1]).1 + 2 * CONTEXT
        {
            until += 1;
        }
        shift = fragment(&old, &hunks[at..until], shift, &mut out);
        at = until;
    }
    Ok(out)
}

/// Writes one fragment of `hunks`, with the old side's lines around and between them, `shift`
/// being what the fragments before it put in less what they took out. Returns the shift after it.
fn fragment(old: &[&[u8]], hunks: &[&Hunk], shift: i64, out: &mut Vec<u8>) -> i64 {
    let start = span(hunks[0]).0.saturating_sub(CONTEXT);
    let end = (span(hunks[hunks.len() - 1]).1 + CONTEXT).min(old.len());
    let removed: usize = hunks.iter().map(|hunk| hunk.removed.len()).sum();
    let added: usize = hunks.iter().map(|hunk| hunk.added.len()).sum();
    let old_count = end - start;
    let new_count = old_count - removed + added;
    // A side of no lines names the line before it, as git writes it.
    let old_first = start + usize::from(old_count > 0);
    let new_first = i64::try_from(start).unwrap_or(i64::MAX) + shift + i64::from(new_count > 0);
    let header = format!("@@ -{old_first},{old_count} +{new_first},{new_count} @@\n");
    out.extend_from_slice(header.as_bytes());
    let mut at = start;
    for hunk in hunks {
        let (from, to) = span(hunk);
        for line in &old[at..from] {
            put(b' ', line, out);
        }
        for line in &hunk.removed {
            put(b'-', line, out);
        }
        for line in &hunk.added {
            put(b'+', line, out);
        }
        at = to;
    }
    for line in &old[at..end] {
        put(b' ', line, out);
    }
    let change = i64::try_from(added).unwrap_or(i64::MAX) - i64::try_from(removed).unwrap_or(0);
    shift + change
}

/// Writes `line` after `sign`, and git's mark for a last line with no line break.
fn put(sign: u8, line: &[u8], out: &mut Vec<u8>) {
    out.push(sign);
    out.extend_from_slice(line);
    if !line.ends_with(b"\n") {
        out.extend_from_slice(b"\n\\ No newline at end of file\n");
    }
}

/// Writes `side` and `path` as one name, quoted as git quotes one when a byte calls for it.
fn quoted(side: &[u8], path: &[u8], out: &mut Vec<u8>) {
    let plain = |byte: &u8| (0x20..0x7f).contains(byte) && *byte != b'"' && *byte != b'\\';
    if path.iter().all(plain) {
        out.extend_from_slice(side);
        out.extend_from_slice(path);
        return;
    }
    out.push(b'"');
    for byte in side.iter().chain(path) {
        match byte {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            byte if plain(byte) => out.push(*byte),
            byte => out.extend_from_slice(format!("\\{byte:03o}").as_bytes()),
        }
    }
    out.push(b'"');
}

#[cfg(test)]
#[path = "stage_tests.rs"]
mod tests;
