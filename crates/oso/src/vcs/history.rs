//! The status's commit sections (ADR 0127, step 2): what `git log -z` gives for the unpushed,
//! unpulled and recent commits, read into rows, and the arguments that read them.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; [`parse`] is a pure function
//! of a child's output, which is untrusted: a record that does not parse is skipped, and nothing
//! here panics or grows past its input.
//!
//! - Each section reads at most [`MAX_COMMITS`] commits, and one more to tell that there are
//!   more; the recent ones [`RECENT`].
//! - A subject is shown on one line, escaped as a file's name is, so that no subject can draw
//!   a row of its own or reorder what is around it.

/// The most commits the unpushed and unpulled sections show.
pub(crate) const MAX_COMMITS: usize = 256;
/// How many recent commits show.
pub(crate) const RECENT: usize = 10;
/// The most bytes of one section's log read: past it, the commits after are not shown.
pub(crate) const MAX_LOG: usize = 1024 * 1024;
/// The most bytes of a subject shown, `%s` being a commit's whole first paragraph: past it, the
/// subject is cut at a character and ends with `…`, as an unpulled commit is anyone's text.
pub(crate) const MAX_SUBJECT: usize = 1024;
/// The `git log` format each record is read with: the full id, the short one and the subject.
pub(crate) const FORMAT: &str = "--format=%H%x1f%h%x1f%s";

/// A commit as a section shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Commit {
    /// Its full id, as hex.
    pub(crate) id: String,
    /// Its short id; for a stash, git's selector, `stash@{N}`, as it was read (step 3b).
    pub(crate) short: String,
    /// Its subject as git wrote it, which may be any bytes.
    pub(crate) subject: Vec<u8>,
}

/// A section's commits, newest first, and whether git listed more than it shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Log {
    pub(crate) commits: Vec<Commit>,
    pub(crate) more: bool,
}

/// Reads `git log -z` output with [`FORMAT`], keeping at most `keep` commits; a record past
/// them says there are more. A record whose ids are not hex is skipped.
pub(crate) fn parse(bytes: &[u8], keep: usize) -> Log {
    let mut log = Log::default();
    for record in bytes.split(|&byte| byte == 0) {
        // `git log -z` ends each record with a NUL, and some versions put a line break first.
        let record = record.strip_prefix(b"\n").unwrap_or(record);
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, |&byte| byte == 0x1f);
        let (Some(id), Some(short), Some(subject)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let hex = |part: &[u8]| {
            !part.is_empty() && part.len() <= 64 && part.iter().all(u8::is_ascii_hexdigit)
        };
        if !hex(id) || !hex(short) || short.len() > id.len() {
            continue;
        }
        if log.commits.len() == keep {
            log.more = true;
            break;
        }
        log.commits.push(Commit {
            id: String::from_utf8_lossy(id).into_owned(),
            short: String::from_utf8_lossy(short).into_owned(),
            subject: cut(subject.strip_suffix(b"\n").unwrap_or(subject)),
        });
    }
    log
}

/// `subject`, cut to [`MAX_SUBJECT`] bytes before a UTF-8 character, with `…` after it if it was.
pub(crate) fn cut(subject: &[u8]) -> Vec<u8> {
    if subject.len() <= MAX_SUBJECT {
        return subject.to_vec();
    }
    let mut end = MAX_SUBJECT;
    // Back past continuation bytes, at most three, to a character's start.
    while end > MAX_SUBJECT - 3 && subject.get(end).is_some_and(|b| b & 0xc0 == 0x80) {
        end -= 1;
    }
    let mut out = subject[..end].to_vec();
    out.extend_from_slice("…".as_bytes());
    out
}

/// `git log`'s arguments for `range`, reading at most `keep` commits and one more.
pub(crate) fn args(range: &str, keep: usize) -> Vec<String> {
    vec![
        "log".into(),
        "-z".into(),
        "--no-show-signature".into(),
        "--no-color".into(),
        FORMAT.into(),
        format!("--max-count={}", keep + 1),
        range.into(),
        "--".into(),
    ]
}

#[cfg(test)]
mod tests;
