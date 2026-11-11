//! What is wrong in a configuration file, said with its line and column, and quoted so that
//! nothing in the file can change how the message reads (ADR 0108).
//!
//! Must not depend on: `app`, `dispatch` or the TOML parser's types; problems are found by
//! byte offset and placed by line and column once, at the end.
//!
//! Only the first [`MAX_PROBLEMS`] in the order of the file are kept, and the rest counted, so a
//! file of any size costs a bounded number of messages. A message is written only if it is kept:
//! callers give [`fmt::Arguments`], and [`quoted`] escapes as it is written.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fmt::{self, Write};

use oso_core::display_map::cell::escape;

/// The most characters of the file's own text that one message quotes (ADR 0108).
pub const MAX_QUOTED: usize = 80;

/// The most problems a load keeps, the first in the order of the file: as many as `*Messages*`
/// shows (ADR 0108). The rest are only counted.
pub const MAX_PROBLEMS: usize = 20;

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

/// A problem kept, by byte offset and then the order it was found in.
#[derive(Debug)]
struct Kept {
    offset: usize,
    found: usize,
    message: String,
}

impl Kept {
    fn place(&self) -> (usize, usize) {
        (self.offset, self.found)
    }
}

impl PartialEq for Kept {
    fn eq(&self, other: &Self) -> bool {
        self.place() == other.place()
    }
}

impl Eq for Kept {}

impl PartialOrd for Kept {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Kept {
    fn cmp(&self, other: &Self) -> Ordering {
        self.place().cmp(&other.place())
    }
}

/// Problems as they are found, by byte offset into the file: the first [`MAX_PROBLEMS`] of
/// them, and how many there are.
#[derive(Debug, Default)]
pub(super) struct Found {
    /// The last kept on top, to give way to one found before it in the file.
    kept: BinaryHeap<Kept>,
    count: usize,
}

impl Found {
    /// A problem at byte `offset`. `what` is written only if the problem is kept.
    pub(super) fn at(&mut self, offset: usize, what: fmt::Arguments<'_>) {
        self.count += 1;
        let full = self.kept.len() == MAX_PROBLEMS;
        if full && self.kept.peek().is_some_and(|last| offset >= last.offset) {
            return;
        }
        self.kept.push(Kept {
            offset,
            found: self.count,
            message: what.to_string(),
        });
        if full {
            self.kept.pop();
        }
    }

    /// A problem with the entry whose keys or name are spelled `subject`, at byte `offset`:
    /// the spelling, quoted, then `what`.
    pub(super) fn about(&mut self, offset: usize, subject: &str, what: fmt::Arguments<'_>) {
        self.at(offset, format_args!("{}: {what}", quoted(subject)));
    }

    /// A syntax error, in the parser's words, at byte `offset`.
    pub(super) fn syntax(&mut self, offset: usize, wording: &str) {
        self.at(
            offset,
            format_args!("syntax error: {}", Escaped::new(wording, MAX_WORDING)),
        );
    }

    /// The problems kept, in the order of the file, each with its line and column, and how many
    /// more there were. One pass over the text, up to the last kept, places them all, however
    /// long a line is.
    pub(super) fn placed(self, text: &str) -> (Vec<Problem>, usize) {
        let more = self.count - self.kept.len();
        let mut chars = text.char_indices().peekable();
        let (mut line, mut column) = (1, 1);
        let problems = self
            .kept
            .into_sorted_vec()
            .into_iter()
            .map(|kept| {
                while let Some((_, c)) = chars.next_if(|(at, _)| *at < kept.offset) {
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
                    message: kept.message,
                }
            })
            .collect();
        (problems, more)
    }
}

/// `text` in double quotes, cut at [`MAX_QUOTED`] characters, with control and invisible
/// characters, bidi controls among them, escaped as the display draws them (`^[`, `U+202E`), and
/// `"` and `\` escaped with a backslash, so the text cannot close its quotes. Nothing is written
/// until the message is.
pub(super) fn quoted<T: fmt::Display>(text: T) -> Quoted<T> {
    Quoted(text)
}

/// See [`quoted`].
pub(super) struct Quoted<T>(T);

impl<T: fmt::Display> fmt::Display for Quoted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", Escaped::new(&self.0, MAX_QUOTED).quoting())
    }
}

/// Text cut at so many characters, with an ellipsis if cut, and escaped as the display escapes
/// control and invisible characters; inside quotes, `"` and `\` too.
struct Escaped<T> {
    text: T,
    max: usize,
    quotes: bool,
}

impl<T> Escaped<T> {
    fn new(text: T, max: usize) -> Self {
        Self {
            text,
            max,
            quotes: false,
        }
    }

    /// Escapes `"` and `\` too, for text inside quotes.
    fn quoting(self) -> Self {
        Self {
            quotes: true,
            ..self
        }
    }
}

impl<T: fmt::Display> fmt::Display for Escaped<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = Escaping {
            out: f,
            left: self.max,
            quotes: self.quotes,
            cut: false,
        };
        write!(out, "{}", self.text)?;
        if out.cut {
            f.write_char('…')?;
        }
        Ok(())
    }
}

/// Writes through to `out`, escaping, until `left` characters are written.
struct Escaping<'a, 'f> {
    out: &'a mut fmt::Formatter<'f>,
    left: usize,
    quotes: bool,
    cut: bool,
}

impl Write for Escaping<'_, '_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for c in text.chars() {
            if self.left == 0 {
                self.cut = true;
                return Ok(());
            }
            self.left -= 1;
            match escape(c) {
                Some(escape) => self.out.write_str(escape.as_str())?,
                None if self.quotes && matches!(c, '"' | '\\') => {
                    self.out.write_char('\\')?;
                    self.out.write_char(c)?;
                }
                None => self.out.write_char(c)?,
            }
        }
        Ok(())
    }
}
