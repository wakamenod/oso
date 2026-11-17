//! What a commit needs that runs no git (ADR 0127, step 1b): the arguments of each kind, the
//! message buffer's first text and its `#` lines, the message once they are left out, its
//! highlights, the ring of messages kept, which hooks a failure names, and git's output as a
//! log shows it.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these are plain functions,
//! tested without git, but for the hooks a failure names, which are looked for on disk on the
//! run thread.
//!
//! - The commit runs `git -c core.commentChar=# commit --cleanup=strip --file=-` with the
//!   message on standard input, so that `#` lines are left out whatever the person's
//!   configuration says, and the switches as flags. Extending runs `--amend --no-edit`, and
//!   rewording `--amend --only --allow-empty`, which leaves what is staged out. A fixup runs
//!   `--fixup=ID`, which writes its own message, and a squash `--squash=ID --file=-`, its
//!   message after git's `squash!` line (step 2); `ID` is a full id git listed.
//! - A file's name in the `#` lines comes from git, so it is shown escaped, on one line: a line
//!   break in it could not end a `#` line and add one of the message's.

use std::collections::VecDeque;
use std::path::Path;

use oso_core::editor::{CommitKind, CommitSwitches, MAX_GIT_LOG, row_text};
use oso_core::syntax::{Highlight, Highlights};

/// The most messages the ring keeps, as magit's `log-edit` ring.
pub(crate) const MAX_RING: usize = 20;
/// The characters a summary line holds before the rest is marked.
pub(crate) const SUMMARY: usize = 50;
/// The characters a body line holds before the rest is marked.
pub(crate) const BODY: usize = 72;
/// The most staged files the `#` lines name.
pub(crate) const MAX_LISTED: usize = 100;
/// The most bytes of a message that is highlighted, each time it changes; a longer one is
/// shown plain. Far past any message, so a key in one costs some microseconds.
pub(crate) const MAX_HIGHLIGHTED: usize = 64 * 1024;
/// The most bytes a message may have: git is not given more.
pub(crate) const MAX_MESSAGE: usize = 1024 * 1024;

/// The hooks a commit runs that can make it fail, in the order git runs them, and whether
/// `--no-verify` leaves each out.
const HOOKS: [(&str, bool); 3] = [
    ("pre-commit", true),
    ("prepare-commit-msg", false),
    ("commit-msg", true),
];

/// git's arguments for the commit `kind` with `switches`, after oso's own, a fixup or a
/// squash for the commit `target`, a full id. A kind that takes a message reads it from
/// standard input.
pub(crate) fn args(
    kind: CommitKind,
    switches: CommitSwitches,
    target: Option<&str>,
) -> Vec<String> {
    let mut words: Vec<String> = ["-c", "core.commentChar=#", "commit", "--cleanup=strip"]
        .map(str::to_owned)
        .to_vec();
    let target = target.unwrap_or_default();
    let own: Vec<String> = match kind {
        CommitKind::New => vec!["--file=-".into()],
        CommitKind::Amend => vec!["--amend".into(), "--file=-".into()],
        CommitKind::Extend => vec!["--amend".into(), "--no-edit".into()],
        // A commit that changes nothing could not be reworded otherwise.
        CommitKind::Reword => ["--amend", "--only", "--allow-empty", "--file=-"]
            .map(str::to_owned)
            .to_vec(),
        CommitKind::Fixup => vec![format!("--fixup={target}")],
        CommitKind::Squash => vec![format!("--squash={target}"), "--file=-".into()],
    };
    words.extend(own);
    if switches.no_verify {
        words.push("--no-verify".into());
    }
    if switches.signoff {
        words.push("--signoff".into());
    }
    if switches.allow_empty && !words.iter().any(|word| word == "--allow-empty") {
        words.push("--allow-empty".into());
    }
    words
}

/// Whether the commit `kind` takes a message the person writes.
pub(crate) fn takes_message(kind: CommitKind) -> bool {
    !matches!(kind, CommitKind::Extend | CommitKind::Fixup)
}

/// Whether the commit `kind` acts on the commit at point (step 2).
pub(crate) fn needs_target(kind: CommitKind) -> bool {
    matches!(kind, CommitKind::Fixup | CommitKind::Squash)
}

/// What the log and the echo area call the commit `kind`.
pub(crate) fn title(kind: CommitKind) -> &'static str {
    match kind {
        CommitKind::New => "git commit",
        CommitKind::Amend => "git commit --amend",
        CommitKind::Extend => "git commit --amend --no-edit",
        CommitKind::Reword => "git commit --amend --only",
        CommitKind::Fixup => "git commit --fixup",
        CommitKind::Squash => "git commit --squash",
    }
}

/// A staged file as the `#` lines name it: its status letter and its path as git wrote it.
pub(crate) struct Staged<'a> {
    pub(crate) letter: char,
    pub(crate) path: &'a [u8],
}

/// The message buffer's first text: `message`, which amending and rewording start from, then
/// the `#` lines, naming the branch and up to [`MAX_LISTED`] of the `staged` files, of
/// `count` in all, and the keys. Rewording names no file, as what is staged stays out.
pub(crate) fn template(
    message: &str,
    branch: Option<&str>,
    staged: &[Staged<'_>],
    count: usize,
    kind: CommitKind,
) -> String {
    let mut text = message.trim_end().to_owned();
    text.push_str("\n\n");
    text.push_str(
        "# Lines starting with '#' are left out, and an empty message commits nothing.\n",
    );
    text.push_str("# C-c C-c commits, C-c C-k cancels, M-p and M-n bring back earlier messages.\n");
    text.push_str("#\n");
    if let Some(branch) = branch {
        text.push_str(&format!("# On branch {}\n", row_text(branch)));
    }
    if kind == CommitKind::Reword {
        text.push_str("# Rewording: what is staged stays out.\n");
        return text;
    }
    if count == 0 {
        text.push_str("# Nothing is staged.\n");
        return text;
    }
    text.push_str("# Staged:\n");
    for file in staged.iter().take(MAX_LISTED) {
        let path = row_text(&String::from_utf8_lossy(file.path));
        text.push_str(&format!("#   {}  {path}\n", file.letter));
    }
    let listed = staged.len().min(MAX_LISTED);
    if count > listed {
        let more = count - listed;
        text.push_str(&format!("#   … and {more} more\n"));
    }
    text
}

/// The message `text` holds, as git's `--cleanup=strip` leaves it: no `#` line, no trailing
/// white space on a line, no blank line first, last, or after another. Empty if nothing is left.
pub(crate) fn message_of(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let line = line.trim_end();
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
            blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The part of `text` before its first `#` line, which `M-p` and `M-n` replace.
pub(crate) fn message_end(text: &str) -> usize {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with('#') {
            return at;
        }
        at += line.len();
    }
    text.len()
}

/// The summary of `message`: its first line.
pub(crate) fn summary(message: &str) -> &str {
    message.lines().next().unwrap_or_default()
}

/// The highlights of a message buffer's `text`: `#` lines as comments, and what passes
/// [`SUMMARY`] characters on the first line and [`BODY`] on the others, with a second line
/// that is not empty, marked. None at all past [`MAX_HIGHLIGHTED`] bytes.
pub(crate) fn highlights(text: &str) -> Highlights {
    let mut highlights = Highlights::new(text.len());
    if text.len() > MAX_HIGHLIGHTED {
        return highlights;
    }
    let mut runs: Vec<(usize, Option<Highlight>)> = Vec::new();
    let mut push = |len: usize, face: Option<Highlight>| match runs.last_mut() {
        Some((run, last)) if *last == face => *run += len,
        _ if len > 0 => runs.push((len, face)),
        _ => {}
    };
    for (row, line) in text.split_inclusive('\n').enumerate() {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let rest = line.len() - body.len();
        if body.starts_with('#') {
            push(body.len(), Some(Highlight::Comment));
        } else if row == 1 {
            push(body.len(), Some(Highlight::GitCommitOverlong));
        } else {
            let keep = if row == 0 { SUMMARY } else { BODY };
            let cut = body
                .char_indices()
                .nth(keep)
                .map_or(body.len(), |(at, _)| at);
            push(cut, None);
            push(body.len() - cut, Some(Highlight::GitCommitOverlong));
        }
        push(rest, None);
    }
    highlights.replace(0..text.len(), runs);
    highlights
}

/// The messages kept, the newest first: each one committed or cancelled.
#[derive(Debug, Default)]
pub(crate) struct Ring {
    kept: VecDeque<String>,
}

impl Ring {
    /// Keeps `message`, unless it is empty or the newest already, as the newest; the oldest
    /// goes past [`MAX_RING`].
    pub(crate) fn keep(&mut self, message: String) {
        if message.trim().is_empty() {
            return;
        }
        self.kept.retain(|kept| *kept != message);
        self.kept.push_front(message);
        self.kept.truncate(MAX_RING);
    }

    /// The message `at` steps back, 0 the newest.
    pub(crate) fn get(&self, at: usize) -> Option<&str> {
        self.kept.get(at).map(String::as_str)
    }

    /// How many messages it keeps.
    pub(crate) fn len(&self) -> usize {
        self.kept.len()
    }

    /// The message `M-p` (`older`) or `M-n` shows from `at`, the one shown, `None` for the
    /// person's own: an older one, a newer one, or none past either end.
    pub(crate) fn step(&self, at: Option<usize>, older: bool) -> Option<usize> {
        let next = match (at, older) {
            (None, true) => Some(0),
            (Some(n), true) => Some(n + 1),
            (Some(n), false) => n.checked_sub(1),
            (None, false) => None,
        };
        next.filter(|&n| n < self.kept.len())
    }
}

/// The hooks in `hooks`, the repository's hooks directory, that a commit runs and that may
/// have made it fail: each that is there with an execute bit, as git runs it, but those
/// `--no-verify` left out.
pub(crate) fn hooks_present(hooks: &Path, no_verify: bool) -> Vec<&'static str> {
    HOOKS
        .iter()
        .filter(|(_, verified)| !(no_verify && *verified))
        .map(|(name, _)| *name)
        .filter(|name| runnable(&hooks.join(name)))
        .collect()
}

/// Whether git would run the hook at `path`: a file with an execute bit, as the process
/// module judges an executable.
fn runnable(path: &Path) -> bool {
    oso_platform::process::Executable::absolute(path).is_ok()
}

/// What the echo area says of a commit that failed, naming the hooks that may have made it.
pub(crate) fn failed(hooks: &[&str]) -> String {
    match hooks {
        [] => "commit failed: $ shows the output".to_owned(),
        [one] => format!("commit failed (hook {one}): $ shows the output"),
        many => format!(
            "commit failed (hook {}): $ shows the output",
            many.join(" or ")
        ),
    }
}

/// git's output as a log shows it, line by line: escape sequences, such as colours, and
/// control characters but tabs taken out, a line rewritten after a carriage return keeping only
/// what came last, and bytes that are not UTF-8 replaced. A line not ended yet waits in
/// `partial` for the next piece or the end: what follows its last carriage return, and at most
/// [`MAX_GIT_LOG`] bytes of that. Each piece costs what it holds, never what waits.
pub(crate) fn log_lines(piece: &[u8], partial: &mut Vec<u8>) -> String {
    // `partial` never holds a line break, so only the piece is looked through for one.
    let Some(last) = piece.iter().rposition(|&byte| byte == b'\n') else {
        keep_partial(partial, piece);
        return String::new();
    };
    partial.extend_from_slice(&piece[..=last]);
    let whole = std::mem::take(partial);
    keep_partial(partial, &piece[last + 1..]);
    let mut out = String::new();
    for line in whole.split_inclusive(|&byte| byte == b'\n') {
        out.push_str(&clean_line(line));
        out.push('\n');
    }
    out
}

/// Adds `bytes`, of a line not ended, to `partial`: what a carriage return in them rewrote goes,
/// but a last one, which may begin `\r\n`; past twice [`MAX_GIT_LOG`], the start goes too.
fn keep_partial(partial: &mut Vec<u8>, bytes: &[u8]) {
    // The one byte before may be a carriage return that the last piece ended with.
    let start = partial.len().saturating_sub(1);
    partial.extend_from_slice(bytes);
    let before_last = partial.len().saturating_sub(1).max(start);
    if let Some(at) = partial[start..before_last]
        .iter()
        .rposition(|&b| b == b'\r')
    {
        partial.drain(..=start + at);
    }
    if partial.len() > 2 * MAX_GIT_LOG {
        let cut = partial.len() - MAX_GIT_LOG;
        partial.drain(..cut);
    }
}

/// The rest of a run's output, its last line not ended, as [`log_lines`] shows it.
pub(crate) fn log_rest(partial: &mut Vec<u8>) -> String {
    if partial.is_empty() {
        return String::new();
    }
    let rest = std::mem::take(partial);
    format!("{}\n", clean_line(&rest))
}

/// One line of output, without its line break, cleaned as [`log_lines`] says.
fn clean_line(line: &[u8]) -> String {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    // What a carriage return rewrote is gone: a progress line shows as it ended.
    let line = line
        .iter()
        .rposition(|&byte| byte == b'\r')
        .map_or(line, |at| &line[at + 1..]);
    let text = String::from_utf8_lossy(line);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape(&mut chars),
            '\t' => out.push(c),
            c if c.is_control() || is_bidi_format(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// Skips the rest of an escape sequence: a CSI one to its final byte, an OSC one to its BEL or
/// string terminator, and otherwise the one character after the escape.
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    return;
                }
            }
        }
        Some(']') => {
            while let Some(c) = chars.next() {
                if c == '\u{7}' {
                    return;
                }
                if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                    chars.next();
                    return;
                }
            }
        }
        _ => {}
    }
}

/// Whether `c` reorders text or marks its direction: a log line must read as it was written.
pub(crate) fn is_bidi_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests;
