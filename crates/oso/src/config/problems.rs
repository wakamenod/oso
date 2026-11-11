//! What is wrong in a configuration file, said with its line and column, and quoted so that
//! nothing in the file can change how the message reads (ADR 0108).
//!
//! Must not depend on: `app`, `dispatch` or the TOML parser's types; problems are found by
//! byte offset and placed by line and column once, at the end.

use std::fmt;

use oso_core::display_map::cell::escape;

/// The most characters of the file's own text that one message quotes (ADR 0108).
pub const MAX_QUOTED: usize = 80;

/// The most characters of the parser's own wording that a syntax error keeps.
const MAX_WORDING: usize = 200;

/// One thing wrong in the file: where, and what. An entry with a problem is skipped, or the
/// whole file is, for a syntax error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The line it is on, from 1.
    pub line: usize,
    /// The column, in characters, from 1.
    pub column: usize,
    /// What is wrong. The file's text in it is quoted, cut and escaped.
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.message)
    }
}

/// Problems as they are found, by byte offset into the file.
#[derive(Debug, Default)]
pub(super) struct Found(Vec<(usize, String)>);

impl Found {
    /// A problem at byte `offset`.
    pub(super) fn at(&mut self, offset: usize, message: String) {
        self.0.push((offset, message));
    }

    /// A problem with the entry whose keys or name are spelled `subject`, at byte `offset`:
    /// the spelling, quoted, then `what`. Nothing is quoted for an entry with no problem.
    pub(super) fn about(&mut self, offset: usize, subject: &str, what: fmt::Arguments<'_>) {
        self.at(offset, format!("{}: {what}", quoted(subject)));
    }

    /// A syntax error, in the parser's words, at byte `offset`.
    pub(super) fn syntax(&mut self, offset: usize, wording: &str) {
        self.at(
            offset,
            format!("syntax error: {}", escaped(wording, MAX_WORDING)),
        );
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The problems in the order of the file, each with its line and column. One pass over
    /// the text places them all, however many there are and however long a line is.
    pub(super) fn placed(mut self, text: &str) -> Vec<Problem> {
        // Stable, so that one place's problems keep the order they were found in.
        self.0.sort_by_key(|(offset, _)| *offset);
        let mut chars = text.char_indices().peekable();
        let (mut line, mut column) = (1, 1);
        self.0
            .into_iter()
            .map(|(offset, message)| {
                while let Some((_, c)) = chars.next_if(|(at, _)| *at < offset) {
                    if c == '\n' {
                        line += 1;
                        column = 1;
                    } else {
                        column += 1;
                    }
                }
                Problem {
                    line,
                    column,
                    message,
                }
            })
            .collect()
    }
}

/// `text` in double quotes, cut at [`MAX_QUOTED`] characters, with control and invisible
/// characters, bidi controls among them, escaped as the display draws them (`^[`, `U+202E`).
pub(super) fn quoted(text: &str) -> String {
    format!("\"{}\"", escaped(text, MAX_QUOTED))
}

/// `text` cut at `max` characters, with an ellipsis if cut, and escaped as [`quoted`] says.
fn escaped(text: &str, max: usize) -> String {
    let mut shown = String::new();
    for (i, c) in text.chars().enumerate() {
        if i == max {
            shown.push('…');
            break;
        }
        match escape(c) {
            Some(escape) => shown.push_str(escape.as_str()),
            None => shown.push(c),
        }
    }
    shown
}
