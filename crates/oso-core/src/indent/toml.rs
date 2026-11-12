//! TOML's indentation rules (ADR 0115) and the lexer they read with: comments, the four
//! kinds of string, and the brackets of arrays and inline tables.
//!
//! Must not depend on: `buffer`, or the tree highlighting parses; it lexes the text itself.
//!
//! Inside a multi-line array or inline table a line goes one level past its opener's line,
//! and a closer at its start back to it. Inside a multi-line string it keeps its
//! indentation. Elsewhere it takes the indentation of the last non-blank line outside any
//! bracket, so a file that indents its tables keeps doing so.

use super::pass::{Head, Lexer, MAX_DEPTH};
use super::{Answer, Settings};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Inside {
    Code,
    /// A multi-line basic string, `"""`.
    Basic,
    /// A multi-line literal string, `'''`.
    Literal,
}

pub(super) struct Toml {
    settings: Settings,
    inside: Inside,
    /// The indentation of the line of each bracket still open.
    open: Vec<usize>,
    /// Brackets open past [`MAX_DEPTH`].
    deeper: usize,
    /// The indentation of the last non-blank line that started outside any bracket.
    outside: usize,
}

impl Lexer for Toml {
    fn fresh(settings: Settings) -> Self {
        Self {
            settings,
            inside: Inside::Code,
            open: Vec::new(),
            deeper: 0,
            outside: 0,
        }
    }

    fn reset(&mut self, settings: Settings) {
        let mut open = std::mem::take(&mut self.open);
        open.clear();
        *self = Self::fresh(settings);
        self.open = open;
    }

    fn is_anchor(line: &[u8]) -> bool {
        line.starts_with(b"[")
    }

    fn is_inside(&self) -> bool {
        self.inside != Inside::Code
    }

    fn line(&mut self, text: &str, indent: usize) {
        let bytes = text.as_bytes();
        let outside = self.inside == Inside::Code && self.open.is_empty() && self.deeper == 0;
        let blank = bytes
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r'));
        if outside && !blank {
            self.outside = indent;
        }
        let mut index = 0;
        while index < bytes.len() {
            index = match self.inside {
                Inside::Basic => self.multi_line(bytes, index, b'"'),
                Inside::Literal => self.multi_line(bytes, index, b'\''),
                Inside::Code => match bytes[index] {
                    b'#' => break,
                    quote @ (b'"' | b'\'') if bytes[index..].starts_with(&[quote; 3]) => {
                        self.inside = if quote == b'"' {
                            Inside::Basic
                        } else {
                            Inside::Literal
                        };
                        index + 3
                    }
                    quote @ (b'"' | b'\'') => one_line(bytes, index + 1, quote),
                    b'[' | b'{' => {
                        if self.deeper > 0 || self.open.len() == MAX_DEPTH {
                            self.deeper = self.deeper.saturating_add(1);
                        } else {
                            self.open.push(indent);
                        }
                        index + 1
                    }
                    b']' | b'}' => {
                        if self.deeper > 0 {
                            self.deeper -= 1;
                        } else {
                            self.open.pop();
                        }
                        index + 1
                    }
                    _ => index + 1,
                },
            };
        }
    }

    fn answer(&self, head: &Head, _new: bool) -> Option<Answer> {
        if self.inside != Inside::Code {
            return Some(Answer::Keep);
        }
        if self.deeper > 0 {
            return None;
        }
        let column = match self.open.last() {
            Some(&indent) if matches!(head.first(), Some(b']' | b'}')) => indent,
            Some(&indent) => indent.saturating_add(self.settings.indent_size()),
            None => self.outside,
        };
        Some(Answer::Column(column))
    }
}

impl Toml {
    /// Lexes inside a multi-line string closed by three `quote`s, and returns where it goes
    /// on. Up to two more quotes before the three belong to the string.
    fn multi_line(&mut self, bytes: &[u8], mut index: usize, quote: u8) -> usize {
        while let Some(&byte) = bytes.get(index) {
            if byte == b'\\' && quote == b'"' {
                index += 2;
            } else if byte == quote && bytes[index..].starts_with(&[quote; 3]) {
                let run = bytes[index..]
                    .iter()
                    .take(5)
                    .take_while(|&&b| b == quote)
                    .count();
                self.inside = Inside::Code;
                return index + run;
            } else {
                index += 1;
            }
        }
        index
    }
}

/// Where a one-line string whose text starts at `index` ends, past its closing `quote`, or
/// the line's end. Only basic strings have escapes.
fn one_line(bytes: &[u8], mut index: usize, quote: u8) -> usize {
    while let Some(&byte) = bytes.get(index) {
        if byte == b'\\' && quote == b'"' {
            index += 2;
        } else if byte == quote {
            return index + 1;
        } else {
            index += 1;
        }
    }
    index
}
