//! Reads one of oso's patches to libghostty-vt before `build.rs` applies it (ADR 0120): it
//! must be a plain unified diff of files that exist in the library's source, each named the
//! same on its `--- a/` and `+++ b/` lines. `build.rs` includes this file, and
//! `manifest_tests` tests it.
//!
//! Must not depend on: anything; it reads a patch's text only.

/// What may stand before a patch's first file: free text, but nothing `patch` would read as a
/// file header of another kind, or as git's headers for renames, copies, modes, new or
/// deleted files and binary data.
const REFUSED: [&str; 13] = [
    "*** ",
    "Index:",
    "+++ ",
    "rename ",
    "copy ",
    "old mode",
    "new mode",
    "new file mode",
    "deleted file mode",
    "similarity index",
    "dissimilarity index",
    "GIT binary patch",
    "Binary files",
];

/// The files `text` changes, in order, or why it is refused.
pub fn changed_files(text: &str) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(old) = line.strip_prefix("--- ") {
            let old = path_in(old, "a/")?;
            let next = lines.next().unwrap_or_default();
            let new = next
                .strip_prefix("+++ ")
                .ok_or_else(|| format!("{next:?} follows `--- a/{old}`"))?;
            let new = path_in(new, "b/")?;
            if new != old {
                return Err(format!("`--- a/{old}` and `+++ b/{new}` name two files"));
            }
            let mut hunks = 0;
            while lines.peek().is_some_and(|l| l.starts_with("@@ ")) {
                let head = lines.next().unwrap_or_default();
                let (mut old_left, mut new_left) = hunk_lengths(head)?;
                while old_left > 0 || new_left > 0 {
                    let body = lines
                        .next()
                        .ok_or_else(|| format!("{new}: a hunk ends early"))?;
                    let (take_old, take_new) = match body.as_bytes().first() {
                        Some(b' ') | None => (1, 1),
                        Some(b'-') => (1, 0),
                        Some(b'+') => (0, 1),
                        Some(b'\\') => (0, 0),
                        Some(_) => return Err(format!("{new}: {body:?} in a hunk")),
                    };
                    old_left = old_left
                        .checked_sub(take_old)
                        .ok_or_else(|| format!("{new}: a hunk is longer than it says"))?;
                    new_left = new_left
                        .checked_sub(take_new)
                        .ok_or_else(|| format!("{new}: a hunk is longer than it says"))?;
                }
                while lines.peek().is_some_and(|l| l.starts_with('\\')) {
                    lines.next();
                }
                hunks += 1;
            }
            if hunks == 0 {
                return Err(format!("{new}: no hunk"));
            }
            files.push(new);
            continue;
        }
        if let Some(head) = REFUSED.iter().find(|head| line.starts_with(**head)) {
            return Err(format!(
                "{line:?}: {head:?} is not a plain change to an existing file"
            ));
        }
        if !files.is_empty() && !line.starts_with("diff ") {
            return Err(format!("{line:?} between two files"));
        }
    }
    if files.is_empty() {
        return Err("no file".to_owned());
    }
    Ok(files)
}

/// The file a `---` or `+++` line names, after `prefix`: relative, inside the source.
fn path_in(field: &str, prefix: &str) -> Result<String, String> {
    let path = field.split('\t').next().unwrap_or(field).trim_end();
    let Some(path) = path.strip_prefix(prefix) else {
        return Err(format!("{path:?} does not start with {prefix}"));
    };
    if path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("{path:?} leaves the library's source"));
    }
    Ok(path.to_owned())
}

/// The old and new line counts of a `@@ -a,b +c,d @@` line; a count left out is 1.
fn hunk_lengths(head: &str) -> Result<(u32, u32), String> {
    let refused = || format!("{head:?} is not a hunk header");
    let mut fields = head.split(' ').skip(1);
    let old = fields
        .next()
        .and_then(|f| f.strip_prefix('-'))
        .ok_or_else(refused)?;
    let new = fields
        .next()
        .and_then(|f| f.strip_prefix('+'))
        .ok_or_else(refused)?;
    let count = |range: &str| -> Result<u32, String> {
        match range.split_once(',') {
            Some((start, count)) => {
                start.parse::<u32>().map_err(|_| refused())?;
                count.parse().map_err(|_| refused())
            }
            None => range.parse::<u32>().map(|_| 1).map_err(|_| refused()),
        }
    };
    Ok((count(old)?, count(new)?))
}
