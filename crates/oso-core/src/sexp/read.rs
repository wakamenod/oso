//! Reading a buffer's text a token at a time, forward or backward, with what each run of it is
//! (ADR 0190): code, a string or a comment, as the highlights or the lexed lines tell.
//!
//! Must not depend on: `window` or `word`; it reads text and runs over a display snapshot.
//!
//! A reader passes space, punctuation and comments, and gives the next string or run of symbol
//! characters as one atom, and each bracket alone. A mark goes with what it follows, so a token
//! ends at a display cluster's end. Every byte of code read costs one from the budget, and a
//! string or comment passed costs what it holds, at most a seek.

use std::ops::Range;

use super::lex::{self, LINE_MAX};
use super::lines::{LexAhead, LexBehind};
use super::{Kind, ScanError, Syntax, Table, charge};
use crate::display_map::cell::is_mark;
use crate::motion::{SEEK_COST, WalkBudget};
use crate::syntax::{Highlight, Runs, RunsBefore};

/// What a reader found, in its own direction: going backward, a closer is entered and an
/// opener left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Token {
    /// A string or a run of symbol characters, now behind the reader.
    Atom,
    /// A bracket that goes one level in.
    Enter,
    /// A bracket that goes one level out.
    Leave,
    /// The end of what it may read.
    End,
}

/// A reader in either direction.
pub(super) trait Read {
    fn token(&mut self) -> Result<Token, ScanError>;

    /// Where it is.
    fn pos(&self) -> usize;

    /// Whether it reads inside a string or a comment, whose ends are a containing
    /// expression's.
    fn confined(&self) -> bool;
}

/// What highlighting calls a run.
fn kind_of(highlight: Option<Highlight>) -> Kind {
    match highlight {
        Some(Highlight::String) => Kind::Str,
        Some(Highlight::Comment) => Kind::Comment,
        _ => Kind::Code,
    }
}

fn is_symbol(table: Table, c: char) -> bool {
    c.is_alphanumeric()
        || c == '_'
        || match table {
            Table::Script => c == '$',
            Table::Css | Table::Html | Table::Yaml => c == '-',
            _ => false,
        }
}

fn is_closer(table: Table, c: char) -> bool {
    matches!(c, ')' | ']' | '}') || (table == Table::Html && c == '>')
}

/// Whether `c` opens a bracket, `next` the character after it: in HTML, a `<` that starts a
/// tag, a comment or a doctype.
fn is_opener(table: Table, c: char, next: Option<char>) -> bool {
    matches!(c, '(' | '[' | '{')
        || (table == Table::Html
            && c == '<'
            && next.is_some_and(|n| n.is_alphabetic() || matches!(n, '/' | '!' | '?')))
}

/// Whether `c` may extend the cluster before it: only a character past ASCII may.
fn extends(c: char) -> bool {
    !c.is_ascii() && is_mark(c)
}

/// The string or comment whose text holds `offset` past its first byte, with what it is: a
/// reader that starts there reads only inside it.
pub(super) fn around(
    syntax: &Syntax<'_>,
    offset: usize,
    budget: &mut WalkBudget,
) -> Result<Option<(Range<usize>, Kind)>, ScanError> {
    charge(budget, SEEK_COST)?;
    if let Some(highlights) = syntax.highlights {
        return Ok(highlights
            .run_at(offset)
            .map(|(range, highlight)| (range, kind_of(highlight)))
            .filter(|(range, kind)| *kind != Kind::Code && range.start < offset));
    }
    if !lex::lexes(syntax.table) || offset == 0 {
        return Ok(None);
    }
    let display = syntax.display;
    let mut start = offset;
    let mut back = Vec::new();
    'back: for piece in display.text_before(offset) {
        for &b in piece.as_bytes().iter().rev() {
            if b == b'\n' {
                break 'back;
            }
            if back.len() == LINE_MAX {
                return Ok(None);
            }
            back.push(b);
            start -= 1;
        }
    }
    back.reverse();
    let mut line = back;
    'ahead: for piece in display.text_in(offset..display.len()) {
        for &b in piece.as_bytes() {
            if line.len() == LINE_MAX {
                return Ok(None);
            }
            line.push(b);
            if b == b'\n' {
                break 'ahead;
            }
        }
    }
    charge(budget, line.len())?;
    let mut runs = Vec::new();
    lex::lex(syntax.table, &line, start, &mut runs);
    Ok(runs
        .into_iter()
        .find(|(range, kind)| *kind != Kind::Code && range.start < offset && offset < range.end))
}

/// Where runs come from going forward.
enum RunsAhead<'a> {
    /// All code.
    Plain,
    Highlighted(Runs<'a>),
    Lexed(Box<LexAhead<'a>>),
}

/// A reader going forward.
pub(super) struct Ahead<'a, 'b> {
    table: Table,
    pieces: std::iter::Peekable<Box<dyn Iterator<Item = &'a str> + 'a>>,
    /// What is left of the piece at hand.
    piece: &'a str,
    pos: usize,
    limit: usize,
    confined: bool,
    runs: RunsAhead<'a>,
    /// The run that holds `pos`, once looked up.
    run: (Range<usize>, Kind),
    budget: &'b mut WalkBudget,
}

impl<'a, 'b> Ahead<'a, 'b> {
    /// A reader from `from` to the buffer's end, or to the end of the string or comment that
    /// holds it.
    pub(super) fn new(
        syntax: &Syntax<'a>,
        from: usize,
        budget: &'b mut WalkBudget,
    ) -> Result<Self, ScanError> {
        let display = syntax.display;
        let (limit, confined, runs) = match around(syntax, from, budget)? {
            Some((range, _)) => (range.end, true, RunsAhead::Plain),
            None => {
                let runs =
                    match syntax.highlights {
                        Some(highlights) => {
                            RunsAhead::Highlighted(highlights.runs(from..display.len()))
                        }
                        None if lex::lexes(syntax.table) => RunsAhead::Lexed(Box::new(
                            LexAhead::new(display, syntax.table, from, budget)?,
                        )),
                        None => RunsAhead::Plain,
                    };
                (display.len(), false, runs)
            }
        };
        let pieces: Box<dyn Iterator<Item = &'a str> + 'a> = Box::new(display.text_in(from..limit));
        Ok(Self {
            table: syntax.table,
            pieces: pieces.peekable(),
            piece: "",
            pos: from,
            limit,
            confined,
            runs,
            run: (from..from, Kind::Code),
            budget,
        })
    }

    fn charge(&mut self, bytes: usize) -> Result<(), ScanError> {
        charge(self.budget, bytes)
    }

    /// The character at `pos`.
    fn peek(&mut self) -> Option<char> {
        while self.piece.is_empty() {
            self.piece = self.pieces.next()?;
        }
        self.piece.chars().next()
    }

    /// The character after the one at `pos`.
    fn peek_second(&mut self) -> Option<char> {
        let mut chars = self.piece.chars();
        chars.next();
        chars
            .next()
            .or_else(|| self.pieces.peek().and_then(|piece| piece.chars().next()))
    }

    fn bump(&mut self, c: char) {
        self.piece = &self.piece[c.len_utf8()..];
        self.pos += c.len_utf8();
    }

    /// What the byte at `pos` is.
    fn kind(&mut self) -> Result<Kind, ScanError> {
        while self.pos >= self.run.0.end {
            let next = match &mut self.runs {
                RunsAhead::Plain => None,
                RunsAhead::Highlighted(runs) => runs
                    .next()
                    .map(|(range, highlight)| (range, kind_of(highlight))),
                RunsAhead::Lexed(lexed) => match lexed.next() {
                    Some((run, cost)) => {
                        charge(self.budget, cost)?;
                        Some(run)
                    }
                    None => None,
                },
            };
            self.run = next.unwrap_or((self.pos..usize::MAX, Kind::Code));
        }
        Ok(if self.pos >= self.run.0.start {
            self.run.1
        } else {
            Kind::Code
        })
    }

    /// Passes the text up to `end`.
    fn skip_to(&mut self, end: usize) {
        while self.pos < end {
            if self.peek().is_none() {
                return;
            }
            let mut take = (end - self.pos).min(self.piece.len());
            while !self.piece.is_char_boundary(take) {
                take += 1;
            }
            self.piece = &self.piece[take..];
            self.pos += take;
        }
    }

    /// Passes the marks after a token, which are its cluster's.
    fn marks(&mut self) -> Result<(), ScanError> {
        while self.pos < self.limit && self.kind()? == Kind::Code {
            match self.peek() {
                Some(c) if extends(c) => {
                    self.charge(c.len_utf8())?;
                    self.bump(c);
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn rest_of_atom(&mut self) -> Result<(), ScanError> {
        while self.pos < self.limit && self.kind()? == Kind::Code {
            match self.peek() {
                Some(c) if is_symbol(self.table, c) || extends(c) => {
                    self.charge(c.len_utf8())?;
                    self.bump(c);
                }
                _ => break,
            }
        }
        Ok(())
    }
}

impl Read for Ahead<'_, '_> {
    fn token(&mut self) -> Result<Token, ScanError> {
        loop {
            if self.pos >= self.limit {
                return Ok(Token::End);
            }
            let kind = self.kind()?;
            if kind != Kind::Code {
                let end = self.run.0.end.min(self.limit);
                self.charge((end - self.pos).min(SEEK_COST))?;
                self.skip_to(end);
                if kind == Kind::Str {
                    return Ok(Token::Atom);
                }
                continue;
            }
            let Some(c) = self.peek() else {
                return Ok(Token::End);
            };
            self.charge(c.len_utf8())?;
            if is_closer(self.table, c) {
                self.bump(c);
                self.marks()?;
                return Ok(Token::Leave);
            }
            let next = if c == '<' { self.peek_second() } else { None };
            if is_opener(self.table, c, next) {
                self.bump(c);
                self.marks()?;
                return Ok(Token::Enter);
            }
            self.bump(c);
            if is_symbol(self.table, c) {
                self.rest_of_atom()?;
                return Ok(Token::Atom);
            }
        }
    }

    fn pos(&self) -> usize {
        self.pos
    }

    fn confined(&self) -> bool {
        self.confined
    }
}

/// Where runs come from going backward.
enum RunsBehind<'a> {
    Plain,
    Highlighted(RunsBefore<'a>),
    Lexed(Box<LexBehind<'a>>),
}

/// A reader going backward.
pub(super) struct Behind<'a, 'b> {
    table: Table,
    pieces: Box<dyn Iterator<Item = &'a str> + 'a>,
    /// What is left of the piece at hand, read from its end.
    piece: &'a str,
    pos: usize,
    limit: usize,
    confined: bool,
    runs: RunsBehind<'a>,
    run: (Range<usize>, Kind),
    /// The character after `pos`, if known.
    after: Option<char>,
    budget: &'b mut WalkBudget,
}

impl<'a, 'b> Behind<'a, 'b> {
    /// A reader from `from` back to the buffer's start, or to the start of the string or
    /// comment that holds it.
    pub(super) fn new(
        syntax: &Syntax<'a>,
        from: usize,
        budget: &'b mut WalkBudget,
    ) -> Result<Self, ScanError> {
        let display = syntax.display;
        let (limit, confined, runs) = match around(syntax, from, budget)? {
            Some((range, _)) => (range.start, true, RunsBehind::Plain),
            None => {
                let runs = match syntax.highlights {
                    Some(highlights) => RunsBehind::Highlighted(highlights.runs_before(from)),
                    None if lex::lexes(syntax.table) => {
                        RunsBehind::Lexed(Box::new(LexBehind::new(display, syntax.table, from)))
                    }
                    None => RunsBehind::Plain,
                };
                (0, false, runs)
            }
        };
        let after = if syntax.table == Table::Html && from < display.len() {
            let (_, end) = display.cluster_at(from);
            display
                .text_in(from..end.max(from))
                .find_map(|piece| piece.chars().next())
        } else {
            None
        };
        Ok(Self {
            table: syntax.table,
            pieces: Box::new(display.text_before(from)),
            piece: "",
            pos: from,
            limit,
            confined,
            runs,
            run: (from..from, Kind::Code),
            after,
            budget,
        })
    }

    fn charge(&mut self, bytes: usize) -> Result<(), ScanError> {
        charge(self.budget, bytes)
    }

    /// The character before `pos`.
    fn peek(&mut self) -> Option<char> {
        while self.piece.is_empty() {
            self.piece = self.pieces.next()?;
        }
        self.piece.chars().next_back()
    }

    fn bump(&mut self, c: char) {
        self.piece = &self.piece[..self.piece.len() - c.len_utf8()];
        self.pos -= c.len_utf8();
        self.after = Some(c);
    }

    /// What the byte before `pos` is.
    fn kind(&mut self) -> Result<Kind, ScanError> {
        while self.pos <= self.run.0.start {
            let next = match &mut self.runs {
                RunsBehind::Plain => None,
                RunsBehind::Highlighted(runs) => runs
                    .next()
                    .map(|(range, highlight)| (range, kind_of(highlight))),
                RunsBehind::Lexed(lexed) => match lexed.next() {
                    Some((run, cost)) => {
                        charge(self.budget, cost)?;
                        Some(run)
                    }
                    None => None,
                },
            };
            self.run = next.unwrap_or((0..self.pos, Kind::Code));
        }
        Ok(if self.pos <= self.run.0.end {
            self.run.1
        } else {
            Kind::Code
        })
    }

    /// Passes the text back to `start`.
    fn skip_to(&mut self, start: usize) {
        while self.pos > start {
            if self.peek().is_none() {
                return;
            }
            let take = (self.pos - start).min(self.piece.len());
            let mut cut = self.piece.len() - take;
            while !self.piece.is_char_boundary(cut) {
                cut -= 1;
            }
            self.pos -= self.piece.len() - cut;
            self.piece = &self.piece[..cut];
        }
        self.after = None;
    }

    fn rest_of_atom(&mut self) -> Result<(), ScanError> {
        while self.pos > self.limit && self.kind()? == Kind::Code {
            match self.peek() {
                Some(c) if is_symbol(self.table, c) || extends(c) => {
                    self.charge(c.len_utf8())?;
                    self.bump(c);
                }
                _ => break,
            }
        }
        Ok(())
    }
}

impl Read for Behind<'_, '_> {
    fn token(&mut self) -> Result<Token, ScanError> {
        loop {
            if self.pos <= self.limit {
                return Ok(Token::End);
            }
            let kind = self.kind()?;
            if kind != Kind::Code {
                let start = self.run.0.start.max(self.limit);
                self.charge((self.pos - start).min(SEEK_COST))?;
                self.skip_to(start);
                if kind == Kind::Str {
                    return Ok(Token::Atom);
                }
                continue;
            }
            let Some(c) = self.peek() else {
                return Ok(Token::End);
            };
            self.charge(c.len_utf8())?;
            let mut after = self.after;
            self.bump(c);
            // A mark goes with the character it follows.
            let mut base = c;
            while extends(base) && self.pos > self.limit && self.kind()? == Kind::Code {
                let Some(b) = self.peek() else { break };
                self.charge(b.len_utf8())?;
                after = Some(base);
                self.bump(b);
                base = b;
            }
            if is_closer(self.table, base) {
                return Ok(Token::Enter);
            }
            if is_opener(self.table, base, after) {
                return Ok(Token::Leave);
            }
            if is_symbol(self.table, base) {
                self.rest_of_atom()?;
                return Ok(Token::Atom);
            }
        }
    }

    fn pos(&self) -> usize {
        self.pos
    }

    fn confined(&self) -> bool {
        self.confined
    }
}
