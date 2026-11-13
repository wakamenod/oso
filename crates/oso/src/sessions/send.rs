//! What the person sends to a session (ADR 0121, Q17): the text a send pastes, put together
//! from the parts a command gave against the session's directory, and why a send is refused.
//!
//! Must not depend on: the terminal, threads or I/O. These are pure functions over text that
//! run on the UI thread, at most [`MAX_BYTES`] at a time; the app pastes what they give, as
//! the person's bracketed paste, not submitted.
//!
//! - `@region` is the place, then the text in a fence longer than any run of backticks in it.
//! - `@file:L-L` is the place only, `path:10-20`, which the agent reads itself.
//! - A note is the person's text as written.
//!
//! A path is relative to the session's directory when it is inside it, and absolute otherwise.
//! Control, bidirectional and invisible characters in it are written as `\u{…}`, so that a file
//! name can neither end a line of the form nor read as another. Language servers' diagnostics
//! (`@diagnostics`, `session-fix-error`) would be one more kind of part, put together here
//! too, once oso has them.

use std::fmt::Write as _;
use std::path::Path;

use oso_core::display_map::cell::is_invisible;
use oso_core::editor::{SendExcerpt, SendPart};

use super::state::{Reason, State};

/// The most a send pastes: 256 KiB (ADR 0121), the commands' own limit.
pub const MAX_BYTES: usize = crate::commands::agent_sessions::MAX_SEND_BYTES;

/// A send is larger than [`MAX_BYTES`]: `bytes` long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLarge {
    pub bytes: usize,
}

/// The text that pastes `parts` into a session that runs in `directory`: each part in turn,
/// with a blank line between two. Refused past [`MAX_BYTES`].
pub fn compose(parts: &[SendPart], directory: &Path) -> Result<String, TooLarge> {
    let mut out = String::new();
    for part in parts {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        match part {
            SendPart::Note(text) => out.push_str(text),
            SendPart::Excerpt(excerpt) => {
                out.push_str(&place(excerpt, directory));
                if let Some(text) = &excerpt.text {
                    let fence = fence(text);
                    let _ = write!(out, "\n{fence}\n{text}");
                    if !text.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&fence);
                }
            }
        }
        if out.len() > MAX_BYTES {
            return Err(TooLarge { bytes: out.len() });
        }
    }
    Ok(out)
}

/// Where `excerpt` is: `path:10-20`, or `path:10` for one line, the path relative to
/// `directory` when it is inside it. A buffer that visits no file is named as a buffer.
pub fn place(excerpt: &SendExcerpt, directory: &Path) -> String {
    let lines = if excerpt.first == excerpt.last {
        excerpt.first.to_string()
    } else {
        format!("{}-{}", excerpt.first, excerpt.last)
    };
    match &excerpt.path {
        Some(path) => {
            let shown = path.strip_prefix(directory).unwrap_or(path);
            format!("{}:{lines}", escaped(&shown.to_string_lossy()))
        }
        None => format!("buffer {}, lines {lines}", escaped(&excerpt.buffer)),
    }
}

/// A fence of backticks longer than any run of them in `text`, and at least three.
pub fn fence(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for byte in text.bytes() {
        if byte == b'`' {
            run += 1;
            longest = usize::max(longest, run);
        } else {
            run = 0;
        }
    }
    "`".repeat(usize::max(3, longest + 1))
}

/// `text` with each control, bidirectional or invisible character written as `\u{…}`.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() || is_invisible(c) {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

/// What the app knows of a session when the person sends to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Standing {
    /// Its `claude` runs.
    pub running: bool,
    /// Its state, or `None` while no look at its screen has given one since it started.
    pub state: Option<State>,
    /// Its terminal does not echo: a password prompt (ADR 0120, Q17).
    pub echo_off: bool,
    /// The last look matched a `hold` rule: a full-screen view, such as a picker or the
    /// settings, holds the state.
    pub held: bool,
}

/// Why nothing may be pasted into the session `name` as it stands, or `None` if a paste may
/// go. A paste into an open dialog could choose one of its options, so a session that needs
/// input is refused, and so are one whose screen no look has judged yet and one a full-screen
/// view holds, where a paste could go to a picker or a setting. No state allows more than a
/// paste the person reads before pressing Enter.
pub fn refusal(name: &str, standing: Standing) -> Option<String> {
    if !standing.running || matches!(standing.state, Some(State::Stopped | State::Failed)) {
        return Some(format!("{name} is not running: session-resume first"));
    }
    if standing.echo_off {
        return Some(format!(
            "{name} waits for a password; nothing is pasted there"
        ));
    }
    let asks = match standing.state {
        None => return Some(format!("{name} is starting; send again in a moment")),
        Some(State::NeedsInput(reason)) => reason,
        Some(_) if standing.held => {
            return Some(format!("{name} shows a full-screen view; close it first"));
        }
        Some(_) => return None,
    };
    let what = match asks {
        Reason::Permission => "is asking for permission",
        Reason::Question => "is asking a question",
        Reason::Plan => "is asking to approve a plan",
        Reason::Elicitation => "has an MCP server asking for input",
        Reason::Dialog => "shows a dialog",
    };
    Some(format!("{name} {what}; answer it first"))
}

#[cfg(test)]
mod tests;
