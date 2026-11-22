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
//! - `@diagnostics` is a line for each of a language server's diagnostics,
//!   `path:line: severity: message`, the message's first line with its blanks as one
//!   (ADR 0138, step 11).
//! - `session-fix-error`'s part is the diagnostic, `path:line:column: severity`, then its
//!   message under a line that says a language server wrote it, in a fence of its own longer
//!   than any run of backticks in it, so that no message can pass as the lines or the ask after
//!   it, then its lines around as `@region` gives lines, then "Fix this error.".
//!
//! A path is relative to the session's directory when it is inside it, and absolute otherwise.
//! Control, bidirectional and invisible characters in it are written as `\u{…}`, so that a file
//! name can neither end a line of the form nor read as another. So are they in a server's
//! message, as the agent channel writes them (`channel::diagnostics::server_text`), but its
//! line breaks, which `session-fix-error` keeps.

use std::fmt::Write as _;
use std::path::Path;

use oso_core::diagnostic::first_line;
use oso_core::display_map::cell::is_invisible;
use oso_core::editor::{SendDiagnostic, SendExcerpt, SendPart};

use crate::channel::diagnostics::server_text;

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
            SendPart::Excerpt(excerpt) => push_excerpt(&mut out, excerpt, directory),
            SendPart::Diagnostics(diagnostics) => {
                for (index, diagnostic) in diagnostics.iter().enumerate() {
                    if index > 0 {
                        out.push('\n');
                    }
                    let first = first_line(&diagnostic.message).split_whitespace();
                    let first = first.collect::<Vec<_>>().join(" ");
                    let _ = write!(
                        out,
                        "{}:{}: {}: {}",
                        named(diagnostic, directory),
                        diagnostic.line,
                        diagnostic.severity.word(),
                        server_text(&first, false)
                    );
                    if out.len() > MAX_BYTES {
                        return Err(TooLarge { bytes: out.len() });
                    }
                }
            }
            SendPart::FixError { diagnostic, around } => {
                let message = server_text(&diagnostic.message, true);
                let fence = fence(&message);
                let _ = writeln!(
                    out,
                    "{}:{}:{}: {}\nThe language server says:\n{fence}\n{message}\n{fence}",
                    named(diagnostic, directory),
                    diagnostic.line,
                    diagnostic.column,
                    diagnostic.severity.word(),
                );
                push_excerpt(&mut out, around, directory);
                out.push_str("\n\nFix this error.");
            }
        }
        if out.len() > MAX_BYTES {
            return Err(TooLarge { bytes: out.len() });
        }
    }
    Ok(out)
}

/// Pushes `excerpt`: its place, then its text in a fence longer than any run of backticks in
/// it, if it has its text.
fn push_excerpt(out: &mut String, excerpt: &SendExcerpt, directory: &Path) {
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

/// The file of `diagnostic`, relative to `directory` when inside it, or its buffer's name.
fn named(diagnostic: &SendDiagnostic, directory: &Path) -> String {
    match &diagnostic.path {
        Some(path) => escaped(
            &path
                .strip_prefix(directory)
                .unwrap_or(path)
                .to_string_lossy(),
        ),
        None => format!("buffer {}", escaped(&diagnostic.buffer)),
    }
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
        None => {
            return Some(format!(
                "{name}'s screen is not judged yet; send again in a moment"
            ));
        }
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

/// What Claude Code's input box holds, as its screen shows it, for the one submit oso makes
/// (`review-ask`, ADR 0163).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputBox {
    /// Nothing typed: the prompt sign alone, or its `Try "…"` hint, whole or cut.
    Empty,
    /// Only a paste the TUI folded, `[Pasted text #N +M lines]`, on one row or wrapped.
    Pasted,
    /// One row of other text at the prompt sign: the person's draft, or an earlier paste.
    Typed,
    /// No input box oso pastes into, and why.
    Other(NoBox),
}

/// Why the screen shows no input box oso pastes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoBox {
    /// No two rules with rows between them, as a full-screen view or a screen not drawn yet
    /// shows, or a TUI that draws its box another way.
    NoRules,
    /// The box's first row does not start with the prompt sign: another mode (`!`, `#`), a
    /// dialog or a picker in its place.
    NoSign,
    /// A dialog's option at the sign, `❯ 1. Yes`, or a slash command's menu, `❯ /…`.
    Menu,
    /// A draft of several rows, or a long one the terminal's width wrapped.
    Rows,
}

impl NoBox {
    /// What the echo area says the session `name` shows, after "nothing was sent" is added.
    pub fn said(self, name: &str) -> String {
        match self {
            Self::NoRules => format!(
                "{name} shows no input box now: no prompt box between two rules on its screen"
            ),
            Self::NoSign => format!(
                "{name} shows no input box now: no prompt sign in its box (a dialog, a picker or \
                 another mode)"
            ),
            Self::Menu => {
                format!("{name} shows a dialog's options or a command menu at its prompt")
            }
            Self::Rows => format!("{name}'s input box holds a draft of several rows"),
        }
    }
}

/// The most rows a folded paste wraps to in a narrow terminal: more is a draft.
const MAX_WRAPPED_PASTE_ROWS: usize = 3;

/// The input box `rows` show, read from the bottom: the rows between the last two rules, the
/// first starting with the prompt sign `❯`. The rules are found as the state rules find them
/// (`screen::last_two_rules`): a run of `─`, or one with the session's name at its right end,
/// as the TUI draws the top rule of a session started with `--name`. A dialog, a picker, a
/// draft of several rows and a mode with another sign all read as [`InputBox::Other`], so that
/// oso pastes nothing there; a folded paste the terminal's width wrapped still reads as one.
pub fn input_box(rows: &[String]) -> InputBox {
    let (Some(above), Some(below)) = super::screen::last_two_rules(rows) else {
        return InputBox::Other(NoBox::NoRules);
    };
    let inside = &rows[above + 1..below];
    let Some(first) = inside.first() else {
        return InputBox::Other(NoBox::NoRules);
    };
    let Some(rest) = first.strip_prefix('❯') else {
        return InputBox::Other(NoBox::NoSign);
    };
    // The sign is followed by a no-break space (U+00A0), which `trim` drops too.
    let rest = rest.trim();
    // A dialog's option at the sign, `❯ 1. Yes`, is no draft, and a slash command's menu
    // shows under `❯ /…`.
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if (digits > 0 && rest[digits..].starts_with(". ")) || rest.starts_with('/') {
        return InputBox::Other(NoBox::Menu);
    }
    if inside.len() > 1 {
        return match wrapped_paste(rest, &inside[1..]) {
            true => InputBox::Pasted,
            false => InputBox::Other(NoBox::Rows),
        };
    }
    if rest.is_empty() || hint(rest) {
        return InputBox::Empty;
    }
    if folded_paste(rest) {
        return InputBox::Pasted;
    }
    InputBox::Typed
}

/// Whether `text` at an empty prompt is its hint, `Try "…"`, whole or cut with `…` in a
/// narrow terminal.
fn hint(text: &str) -> bool {
    text.strip_prefix("Try \"")
        .is_some_and(|hinted| hinted.ends_with('"') || hinted.ends_with('…'))
}

/// Whether `first`, the text at the sign, and the box's rows `more` under it are one folded
/// paste the terminal's width wrapped, at a space or within a word.
fn wrapped_paste(first: &str, more: &[String]) -> bool {
    let ends = more.last().is_some_and(|row| row.trim_end().ends_with(']'));
    if more.len() + 1 > MAX_WRAPPED_PASTE_ROWS || !first.starts_with("[Pasted") || !ends {
        return false;
    }
    let pieces = || std::iter::once(first).chain(more.iter().map(|row| row.trim()));
    folded_paste(&pieces().collect::<Vec<_>>().join(" "))
        || folded_paste(&pieces().collect::<String>())
}

/// Whether `text` is all one paste the TUI folded: `[Pasted text #N]` or
/// `[Pasted text #N +M lines]`.
fn folded_paste(text: &str) -> bool {
    let Some(inner) = (text.strip_prefix("[Pasted text #")).and_then(|t| t.strip_suffix(']'))
    else {
        return false;
    };
    let (number, lines) = match inner.split_once(" +") {
        Some((number, lines)) => (number, lines.strip_suffix(" lines")),
        None => (inner, Some("1")),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    digits(number) && lines.is_some_and(digits)
}

#[cfg(test)]
mod tests;
