//! A repository's stashes (ADR 0127, step 3b): what `git stash list` gives, read into the rows
//! of the status's Stashes section, and what applying, popping, dropping and pushing run and
//! say.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; these are pure functions of a
//! child's output, which is untrusted: a record that does not parse is skipped, and nothing
//! here panics or grows past its input.
//!
//! - Stashes are one stack per repository, shared by every worktree and every agent session in
//!   it (Q5): all show, each with git's `On BRANCH:` text, and each is acted on by its commit's
//!   id. Its `stash@{N}` is found again from that id just before git runs, as another may have
//!   pushed or dropped one since; one gone is refused.
//! - A subject is shown on one line, escaped as a commit's is.

use super::history::{self, Commit, Log};

/// The most stashes the section shows.
pub(crate) const MAX_STASHES: usize = 256;
/// The most bytes of a stash message pushed: one line, as `git stash list` shows it.
pub(crate) const MAX_MESSAGE: usize = 1024;
/// The `git stash list` format each record is read with: the full id, the selector
/// (`stash@{N}`) and the subject.
const FORMAT: &str = "--format=%H%x1f%gd%x1f%gs";

/// What a stash's run does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    Apply,
    Pop,
    Drop,
}

impl Act {
    fn verb(self) -> &'static str {
        match self {
            Self::Apply => "apply",
            Self::Pop => "pop",
            Self::Drop => "drop",
        }
    }

    /// What the echo area says once it went well.
    pub(crate) fn done(self, selector: &str) -> String {
        match self {
            Self::Apply => format!("Applied {selector}; it is kept"),
            Self::Pop => format!("Popped {selector}"),
            Self::Drop => format!("Dropped {selector}"),
        }
    }
}

/// `git stash list`'s arguments, reading at most [`MAX_STASHES`] and one more.
pub(crate) fn list_args() -> Vec<String> {
    vec![
        "stash".into(),
        "list".into(),
        "-z".into(),
        "--no-show-signature".into(),
        "--no-color".into(),
        FORMAT.into(),
        format!("--max-count={}", MAX_STASHES + 1),
    ]
}

/// Reads `git stash list -z` output with its format, keeping at most `keep` stashes, each with
/// its selector as its short name. A record whose id is not hex or whose selector is not
/// `stash@{N}` is skipped.
pub(crate) fn parse(bytes: &[u8], keep: usize) -> Log {
    let mut log = Log::default();
    for record in bytes.split(|&byte| byte == 0) {
        let record = record.strip_prefix(b"\n").unwrap_or(record);
        let mut parts = record.splitn(3, |&byte| byte == 0x1f);
        let (Some(id), Some(selector), Some(subject)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let hex = matches!(id.len(), 40 | 64) && id.iter().all(u8::is_ascii_hexdigit);
        let Some(selector) = std::str::from_utf8(selector)
            .ok()
            .filter(|s| is_selector(s))
        else {
            continue;
        };
        if !hex {
            continue;
        }
        if log.commits.len() == keep {
            log.more = true;
            break;
        }
        log.commits.push(Commit {
            id: String::from_utf8_lossy(id).into_owned(),
            short: selector.to_owned(),
            subject: history::cut(subject.strip_suffix(b"\n").unwrap_or(subject)),
        });
    }
    log
}

/// Whether `text` is a stash's selector, `stash@{N}`.
pub(crate) fn is_selector(text: &str) -> bool {
    text.strip_prefix("stash@{")
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some_and(|n| !n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The selector of the stash whose id is `id` in `listed`, `git stash list -z --format=%H`'s
/// output, newest first: where it is now.
pub(crate) fn find(listed: &[u8], id: &str) -> Option<String> {
    (listed.split(|&byte| byte == 0 || byte == b'\n'))
        .map(<[u8]>::trim_ascii)
        .filter(|line| !line.is_empty())
        .position(|line| line == id.as_bytes())
        .map(|at| format!("stash@{{{at}}}"))
}

/// The arguments of `act` on the stash now at `selector`.
pub(crate) fn args(act: Act, selector: &str) -> Vec<String> {
    vec!["stash".into(), act.verb().into(), selector.into()]
}

/// What the log and the echo area call `act` on `selector`.
pub(crate) fn title(act: Act, selector: &str) -> String {
    format!("git stash {} {selector}", act.verb())
}

/// The arguments of a push, with untracked files if `untracked`, and `message` made one line:
/// control characters become spaces, and it is cut at [`MAX_MESSAGE`] bytes.
pub(crate) fn push_args(untracked: bool, message: &str) -> Vec<String> {
    let mut words: Vec<String> = vec!["stash".into(), "push".into()];
    if untracked {
        words.push("--include-untracked".into());
    }
    let line: String = (message.chars())
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = line.trim();
    if !line.is_empty() {
        let end = line.floor_char_boundary(MAX_MESSAGE.min(line.len()));
        words.push(format!("--message={}", &line[..end]));
    }
    words
}

/// What the echo area says when `title` failed with `output`: a stash that did not apply
/// cleanly says what to do, and that it is kept.
pub(crate) fn failed(act: Act, title: &str, output: &str) -> String {
    let conflict = (output.lines().map(str::trim_start)).any(|line| line.starts_with("CONFLICT ("));
    match act {
        Act::Apply | Act::Pop if conflict => format!(
            "{title} did not apply cleanly, and the stash is kept: resolve each file, s stages it"
        ),
        _ => super::remote::failed(title, output),
    }
}

/// The question before dropping `selector`, `subject` escaped on one line.
pub(crate) fn drop_question(selector: &str, subject: &[u8]) -> String {
    let subject = oso_core::editor::row_text(&String::from_utf8_lossy(subject));
    format!("Drop {selector} {subject}? Its changes can then be found only with git fsck")
}

#[cfg(test)]
mod tests;
