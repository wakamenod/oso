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

use oso_core::display_map::cell::is_invisible;
use oso_core::editor::{CommitKind, row_text};

use super::commit::{is_bidi_format, message_end};

/// The most bytes a draft may have: what the channel takes of one.
pub(crate) use crate::channel::protocol::sessions::MAX_DRAFT;
/// The most lines of a failed run's output a fix request holds: its last.
pub(crate) const MAX_FIX_LINES: usize = 200;
/// The most bytes of a failed run's output a fix request holds: its last.
pub(crate) const MAX_FIX_BYTES: usize = 64 * 1024;
/// The most bytes of the person's own text a draft keeps below it as `#` lines: past it, the
/// draft is refused, so that a crafted message never doubles past what git is given.
pub(crate) const MAX_KEPT: usize = 64 * 1024;
/// The longest run of backticks a fix request keeps of the output, so that its fence stays
/// short whatever the output holds.
const MAX_TICKS: usize = 15;
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
    let listed: Vec<String> = (subjects.iter().take(SUBJECTS))
        .map(|subject| format!("- {}\n", one_line(subject, MAX_SUBJECT)))
        .collect();
    let listed = if listed.is_empty() {
        "- (none yet)\n".to_owned()
    } else {
        listed.concat()
    };
    // The subjects are the repository's text: fenced and named as such, never instructions.
    let fence = fence_for(&listed);
    let mut text = format!(
        "Draft a commit message for {what}.\nFollow this repository's style; its last subjects, the repository's text, are:\n{fence}\n{listed}{fence}\n"
    );
    text.push_str(&format!(
        "Do not commit. Give the message to oso with the tool draft_set (kind: commit, id: {id})."
    ));
    text
}

/// The request to fix a commit in `worktree` that failed, naming `hooks`, with the end of
/// `output`, what the run wrote, less its lines that may hold a secret; and how many of those
/// went.
pub(crate) fn fix_request(worktree: &str, hooks: &[&str], output: &str) -> (String, usize) {
    let hook = match hooks {
        [] => String::new(),
        [one] => format!(" (hook {one})"),
        many => format!(" (hook {})", many.join(" or ")),
    };
    let (kept, dropped) = withhold::withheld(last_lines(output, MAX_FIX_LINES, MAX_FIX_BYTES));
    let end = short_ticks(&kept);
    let fence = fence_for(&end);
    let left_out = match dropped {
        0 => String::new(),
        1 => "\n1 line that may hold a secret was left out.".to_owned(),
        n => format!("\n{n} lines that may hold secrets were left out."),
    };
    let text = format!(
        "`git commit` failed in {worktree}{hook}. Its output, last {MAX_FIX_LINES} lines:\n{fence}\n{}\n{fence}{left_out}\nFix the cause. Do not commit, do not skip hooks, and do not amend; I will commit again.",
        end.trim_end_matches('\n')
    );
    (text, dropped)
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

/// Cuts `text` to its last `bytes` bytes, from a line's start, as [`last_lines`] keeps them.
pub(crate) fn keep_end(text: &mut String, bytes: usize) {
    let len = last_lines(text, usize::MAX, bytes).len();
    text.drain(..text.len() - len);
}

/// A draft request's id: 32 random bits, as 8 hex digits. The system's source is a file, so
/// this runs on the repository's run thread, never the UI's. Refused, with why, where oso has
/// no source of random bytes yet (Windows), as a hosted session's own id is.
pub(crate) fn request_id() -> Result<String, String> {
    let mut bytes = [0u8; 4];
    match oso_platform::random::fill(&mut bytes) {
        Ok(()) => Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Err(
            "A draft request needs random bytes for its id, which oso has only on Unix yet"
                .to_owned(),
        ),
        Err(e) => Err(format!("Cannot make a request id: {e}")),
    }
}

/// `text` with each run of backticks past [`MAX_TICKS`] cut to that many.
fn short_ticks(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(&"`".repeat(MAX_TICKS + 1)) {
        return text.into();
    }
    let mut out = String::with_capacity(text.len());
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        if run <= MAX_TICKS {
            out.push(c);
        }
    }
    out.into()
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
/// bidirectional formatting characters, but tabs, taken out, and invisible ones but the joiners
/// and variation selectors that make an emoji; blank lines at its start and end go, and a run of
/// them, or of lines of white space, is one, so that no text hides below the screen. Refused
/// past [`MAX_DRAFT`] bytes, or empty.
pub(crate) fn cleaned(draft: &str) -> Result<String, String> {
    if draft.len() > MAX_DRAFT {
        return Err(format!("a draft takes at most {} KiB", MAX_DRAFT / 1024));
    }
    let text = draft.replace("\r\n", "\n").replace('\r', "\n");
    let text: String = text
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !(c.is_control() || hidden(c)))
        .collect();
    let mut out = String::with_capacity(text.len());
    let mut blank = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
            blank = false;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    if out.is_empty() {
        return Err("the draft is empty".to_owned());
    }
    out.pop();
    Ok(out)
}

/// What a draft from `name` does to a message buffer that holds `now`: the end of the text it
/// replaces, from the start, and what goes there. The person's own text there, if any, is kept
/// below the draft as `#` lines, under a line that says whose draft came; past [`MAX_KEPT`]
/// bytes of it, the draft is refused.
pub(crate) fn with_draft(now: &str, draft: &str, name: &str) -> Result<(usize, String), String> {
    let end = message_end(now);
    let mine = now[..end].trim();
    if mine.len() > MAX_KEPT {
        return Err(format!(
            "the message holds more than {} KiB of the person's own text; nothing was changed",
            MAX_KEPT / 1024
        ));
    }
    let lines = mine.lines().count();
    let mut put = String::with_capacity(draft.len() + mine.len() + 3 * lines + 128);
    put.push_str(draft);
    put.push_str("\n\n");
    if !mine.is_empty() {
        put.push_str("# your text before the draft from ");
        put.push_str(&row_text(&one_line(name, 80)));
        put.push_str(":\n");
        for line in mine.lines() {
            put.push_str("# ");
            put.push_str(line);
            put.push('\n');
        }
        put.push_str("#\n");
    }
    Ok((end, put))
}

/// Whether `c`, in a draft, would hide or reorder text: a bidi control, or an invisible code
/// point but a joiner or a variation selector, or a tag.
fn hidden(c: char) -> bool {
    let keeps = matches!(c, '\u{200C}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}');
    is_bidi_format(c) || (is_invisible(c) && !keeps) || ('\u{E0000}'..='\u{E0FFF}').contains(&c)
}

/// `text` on one line, its control and bidi characters escaped, cut to `max` characters with
/// `…`.
pub(crate) fn one_line(text: &str, max: usize) -> String {
    let shown = row_text(text);
    match shown.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &shown[..at]),
        None => shown,
    }
}

mod withhold;

#[cfg(test)]
mod tests;
