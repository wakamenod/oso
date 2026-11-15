//! One file's patch, as `git diff -U3 -- PATH` writes it, cut into its header and hunks, and
//! the patch that stages one hunk (ADR 0127).
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; these are pure functions of a
//! child's output, which is untrusted.
//!
//! - Staging applies git's own bytes: a hunk's patch is the file's header and that hunk as git
//!   wrote them, so that `git apply` gets exactly what was shown, and refuses it if the file
//!   moved on.
//! - A hunk whose lines do not add up to its header's counts is shown but cannot be staged
//!   alone, and neither can a hunk of a patch git cut short, nor one of a new or deleted
//!   file, a rename, a binary file or a submodule: those stage as a whole.

/// One hunk: its header line, `@@ -a,b +c,d @@ …`, and its lines, each with its line break.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hunk {
    pub(crate) header: Vec<u8>,
    /// Where it starts on the new side, from 1.
    pub(crate) new_start: u64,
    /// Its lines, each starting with ` `, `-`, `+` or `\`.
    pub(crate) lines: Vec<Vec<u8>>,
    /// Whether its lines add up to its header's counts.
    pub(crate) whole: bool,
}

impl Hunk {
    /// The line of the new side that line `index` of the hunk shows, or, for a removed line,
    /// the new side's line it was removed before, from 1.
    pub(crate) fn new_line(&self, index: usize) -> u64 {
        let before = self.lines.iter().take(index);
        let kept = before
            .filter(|line| matches!(line.first(), Some(b' ' | b'+')))
            .count();
        self.new_start.max(1) + kept as u64
    }
}

/// One file's patch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Patch {
    /// The lines before the first hunk: `diff --git`, `index`, `---` and `+++`.
    pub(crate) header: Vec<u8>,
    pub(crate) hunks: Vec<Hunk>,
    /// Why its hunks cannot be staged one by one, if they cannot.
    pub(crate) whole_only: Option<&'static str>,
    /// Whether git wrote more than was read: the hunks after the last whole one are left out.
    pub(crate) cut: bool,
}

impl Patch {
    /// The bytes it holds.
    pub(crate) fn len(&self) -> usize {
        let hunks = self
            .hunks
            .iter()
            .map(|hunk| hunk.header.len() + hunk.lines.iter().map(Vec::len).sum::<usize>());
        self.header.len() + hunks.sum::<usize>()
    }

    /// The patch that stages hunk `index` alone, or unstages it when applied in reverse: the
    /// header and the hunk as git wrote them. `None` if it cannot be staged alone.
    pub(crate) fn hunk_patch(&self, index: usize) -> Option<Vec<u8>> {
        if self.whole_only.is_some() {
            return None;
        }
        let hunk = self.hunks.get(index).filter(|hunk| hunk.whole)?;
        let mut out = self.header.clone();
        out.extend_from_slice(&hunk.header);
        for line in &hunk.lines {
            out.extend_from_slice(line);
        }
        Some(out)
    }
}

/// Reads a patch of one file, of which `cut` says git wrote more than `bytes`.
pub(crate) fn parse(bytes: &[u8], cut: bool) -> Patch {
    let mut patch = Patch {
        cut,
        ..Patch::default()
    };
    let mut lines = lines(bytes).peekable();
    while let Some(line) = lines.next_if(|line| !line.starts_with(b"@@ ")) {
        patch.header.extend_from_slice(line);
    }
    while let Some(header) = lines.next() {
        let Some(counts) = counts(header) else {
            // Not a hunk's header: a combined diff's, or text that is no patch's.
            patch.whole_only = Some("git's patch for it cannot be cut into hunks");
            break;
        };
        let mut body = Vec::new();
        while let Some(line) = lines.next_if(|line| !line.starts_with(b"@@ ")) {
            body.push(line.to_vec());
        }
        let whole = adds_up(&body, counts) && body.last().is_some_and(|line| line.ends_with(b"\n"));
        patch.hunks.push(Hunk {
            header: header.to_vec(),
            new_start: counts.2,
            lines: body,
            whole,
        });
    }
    if cut {
        // The last hunk may stop part way; it is left out rather than shown short.
        if patch.hunks.last().is_some_and(|hunk| !hunk.whole) {
            patch.hunks.pop();
        }
    }
    patch.whole_only = patch.whole_only.or_else(|| whole_only(&patch.header, cut));
    patch
}

/// Why a patch with `header` stages only as a whole, if it does.
fn whole_only(header: &[u8], cut: bool) -> Option<&'static str> {
    let has = |prefix: &[u8]| lines(header).any(|line| line.starts_with(prefix));
    // A submodule's mode, 160000, ends its `index` line.
    let submodule =
        lines(header).any(|line| line.starts_with(b"index ") && line.ends_with(b" 160000\n"));
    if has(b"diff --cc ") || has(b"diff --combined ") || has(b"@@@ ") {
        Some("a combined diff")
    } else if submodule {
        Some("a submodule")
    } else if has(b"Binary files ") || has(b"GIT binary patch") {
        Some("a binary file")
    } else if has(b"new file mode") {
        Some("a new file")
    } else if has(b"deleted file mode") {
        Some("a deleted file")
    } else if has(b"old mode ") || has(b"new mode ") {
        // A change of mode is in the header, which no hunk shows: it goes only with its file.
        Some("a change of mode")
    } else if has(b"rename from") || has(b"copy from") {
        Some("a renamed file")
    } else if cut {
        Some("its patch is past what oso reads")
    } else {
        None
    }
}

/// The lines of `bytes`, each with its line break; the last may have none.
fn lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split_inclusive(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
}

/// Whether `body` holds as many old and new lines as the header's counts say.
fn adds_up(body: &[Vec<u8>], (_, old, _, new): (u64, u64, u64, u64)) -> bool {
    let mut olds = 0u64;
    let mut news = 0u64;
    for line in body {
        match line.first() {
            Some(b' ') => {
                olds += 1;
                news += 1;
            }
            Some(b'-') => olds += 1,
            Some(b'+') => news += 1,
            Some(b'\\') => {}
            _ => return false,
        }
    }
    olds == old && news == new
}

/// The counts of a hunk's header `@@ -a[,b] +c[,d] @@`: old start and length, new start and
/// length, a length left out being 1.
fn counts(header: &[u8]) -> Option<(u64, u64, u64, u64)> {
    let rest = header.strip_prefix(b"@@ -")?;
    let end = rest.windows(3).position(|window| window == b" @@")?;
    let ranges = std::str::from_utf8(&rest[..end]).ok()?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |text: &str| -> Option<(u64, u64)> {
        let (start, len) = text.split_once(',').unwrap_or((text, "1"));
        let number = |digits: &str| {
            (!digits.is_empty() && digits.len() <= 19 && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| digits.parse().ok())
                .flatten()
        };
        Some((number(start)?, number(len)?))
    };
    let ((old_start, old_len), (new_start, new_len)) = (range(old)?, range(new)?);
    Some((old_start, old_len, new_start, new_len))
}

#[cfg(test)]
mod tests;
