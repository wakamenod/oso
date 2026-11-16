//! What a commit asks of the worktree's agent session (ADR 0127, step 6a): the request for a
//! draft of the message, the request to fix a failed commit, and what a draft given back with
//! `draft_set` becomes in the message buffer.
//!
//! Must not depend on: `app`, `dispatch`, git, sessions or the file system; these are pure
//! functions over text, tested without a session.
//!
//! - The requests are oso's fixed texts, with the worktree, the last subjects and the failed
//!   run's output in them. Output from git and its hooks is the repository's text: its escape
//!   sequences and control characters were taken out as the log took it, and it goes in a fence
//!   longer than any run of backticks it holds, so that nothing in it ends the fence.
//! - A draft is the agent's text: cleaned as a paste is (no control or bidi characters, line
//!   breaks made `\n`), at most [`MAX_DRAFT`] bytes. It takes the place of the text above the
//!   `#` lines; what the person had written there is kept below, as `#` lines, so that it is not
//!   committed unless the person takes it back (the author's choice).

use oso_core::editor::{CommitKind, row_text};

use super::commit::message_end;

/// The most bytes a draft may have.
pub(crate) const MAX_DRAFT: usize = 16 * 1024;
/// The most lines of a failed run's output a fix request holds: its last.
pub(crate) const MAX_FIX_LINES: usize = 200;
/// The most bytes of a failed run's output a fix request holds: its last.
pub(crate) const MAX_FIX_BYTES: usize = 64 * 1024;
/// How many of the last subjects a draft request names.
pub(crate) const SUBJECTS: usize = 5;
/// The most characters of one subject a request names.
const MAX_SUBJECT: usize = 120;

/// The request for a draft of the message of the commit `kind` in `worktree`, whose last
/// subjects are `subjects`, to be given back with `draft_set` and `id`.
pub(crate) fn draft_request(
    kind: CommitKind,
    worktree: &str,
    subjects: &[String],
    id: &str,
) -> String {
    let what = match kind {
        CommitKind::New | CommitKind::Extend => {
            format!("the staged changes in {worktree} (`git diff --cached`)")
        }
        CommitKind::Amend => format!(
            "the last commit and the staged changes in {worktree} (`git show HEAD`, `git diff --cached`)"
        ),
        CommitKind::Reword => format!("the last commit in {worktree} (`git show HEAD`)"),
    };
    let mut text = format!(
        "Draft a commit message for {what}.\nFollow this repository's style; its last subjects are:\n"
    );
    for subject in subjects.iter().take(SUBJECTS) {
        let subject = one_line(subject, MAX_SUBJECT);
        text.push_str(&format!("- {subject}\n"));
    }
    if subjects.is_empty() {
        text.push_str("- (none yet)\n");
    }
    text.push_str(&format!(
        "Do not commit. Give the message to oso with the tool draft_set (kind: commit, id: {id})."
    ));
    text
}

/// The request to fix a commit in `worktree` that failed, naming `hooks`, with the end of
/// `output`, what the run wrote.
pub(crate) fn fix_request(worktree: &str, hooks: &[&str], output: &str) -> String {
    let hook = match hooks {
        [] => String::new(),
        [one] => format!(" (hook {one})"),
        many => format!(" (hook {})", many.join(" or ")),
    };
    let end = last_lines(output, MAX_FIX_LINES, MAX_FIX_BYTES);
    let fence = fence_for(end);
    format!(
        "`git commit` failed in {worktree}{hook}. Its output, last {MAX_FIX_LINES} lines:\n{fence}\n{}\n{fence}\nFix the cause. Do not commit, do not skip hooks, and do not amend; I will commit again.",
        end.trim_end_matches('\n')
    )
}

/// The last `lines` lines of `text`, at most `bytes` of them, from a line's start: a line
/// longer than `bytes` alone keeps its end.
pub(crate) fn last_lines(text: &str, lines: usize, bytes: usize) -> &str {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let from = (body.rmatch_indices('\n').nth(lines.saturating_sub(1))).map_or(0, |(at, _)| at + 1);
    let tail = &text[from..];
    if tail.len() <= bytes {
        return tail;
    }
    let cut = tail.ceil_char_boundary(tail.len() - bytes);
    match tail[cut..].find('\n') {
        Some(at) if cut + at + 1 < tail.len() => &tail[cut + at + 1..],
        _ => &tail[cut..],
    }
}

/// A fence of backticks longer than any run of them in `text`, at least three.
pub(crate) fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// A draft as the agent gave it, cleaned as a paste is: line breaks made `\n`, and control and
/// bidirectional formatting characters, but tabs, taken out; trailing blank lines too. Refused
/// past [`MAX_DRAFT`] bytes, or empty.
pub(crate) fn cleaned(draft: &str) -> Result<String, String> {
    if draft.len() > MAX_DRAFT {
        return Err(format!("a draft takes at most {} KiB", MAX_DRAFT / 1024));
    }
    let text = draft.replace("\r\n", "\n").replace('\r', "\n");
    let text: String = text
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !(c.is_control() || is_bidi_format(c)))
        .collect();
    let text = text.trim_end().to_owned();
    if text.trim().is_empty() {
        return Err("the draft is empty".to_owned());
    }
    Ok(text)
}

/// What a draft from `name` does to a message buffer that holds `now`: the end of the text it
/// replaces, from the start, and what goes there. The person's own text there, if any, is kept
/// below the draft as `#` lines, under a line that says whose draft came.
pub(crate) fn with_draft(now: &str, draft: &str, name: &str) -> (usize, String) {
    let end = message_end(now);
    let mine = now[..end].trim();
    let mut put = format!("{draft}\n\n");
    if !mine.is_empty() {
        put.push_str(&format!(
            "# your text before the draft from {}:\n",
            row_text(&one_line(name, 80))
        ));
        for line in mine.lines() {
            put.push_str(&format!("# {line}\n"));
        }
        put.push_str("#\n");
    }
    (end, put)
}

/// `text` on one line, its control characters escaped, cut to `max` characters with `…`.
fn one_line(text: &str, max: usize) -> String {
    let shown = row_text(text);
    match shown.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &shown[..at]),
        None => shown,
    }
}

/// Whether `c` reorders text or marks its direction.
fn is_bidi_format(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests;
