//! Rust's indentation rules (ADR 0115), over the tokens their lexer (`lex`) finds: line and
//! nested block comments, strings and byte strings with escapes, raw strings with any number
//! of `#`, `c"..."`, char literals against lifetimes, and the brackets `{ ( [`.
//!
//! Must not depend on: `buffer`, or the tree highlighting parses; it lexes the text itself.
//!
//! A line inside a string keeps its indentation. A line that starts with a closer goes to
//! the indentation of its opener's line, and any other line inside a bracket one level
//! past it (block indent, as rustfmt writes). A line that goes on with the statement of the
//! code line before takes one level more, as rustfmt lays out an expression over lines: a
//! chain's calls sit together, one level past the line their chain starts on; an operand
//! after `&&`, `+` or `|` sits with the one before it; and what follows a line of closers,
//! such as `})` then `.await`, sits with the closers.

mod lex;

use super::pass::{Head, Lexer, starts_with_word};
use super::{Answer, Settings};

/// The words a line starts with at column 0 that make it an anchor, beside `macro_rules!`,
/// `#[`, `#!` and `}`.
const ITEM_WORDS: [&[u8]; 14] = [
    b"fn", b"pub", b"impl", b"mod", b"use", b"struct", b"enum", b"trait", b"const", b"static",
    b"type", b"extern", b"unsafe", b"async",
];

/// The bytes an operator at a line's start is made of, as in `&& x`, `|| x`, `+ x`, `== x`.
const OPERATOR_BYTES: &[u8] = b"&|+-*/%^=<>!";

/// What the lexer is inside at a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Inside {
    Code,
    /// A string, byte string or C string, with escapes.
    Str,
    /// A raw string closed by `"` and this many `#`.
    Raw(usize),
    /// Block comments nested `depth` deep, the outermost opened at `column`.
    Comment {
        depth: u32,
        column: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Brace,
    Paren,
    Bracket,
}

/// A bracket still open.
#[derive(Clone, Copy, Debug)]
struct Open {
    kind: Kind,
    /// The indentation of the line that holds it.
    indent: usize,
    /// The line that holds it, counted from where the pass started.
    line: u64,
    /// Whether it opens an attribute, `#[` or `#![`.
    attribute: bool,
    /// Whether a `where` clause is open directly inside it.
    clause: bool,
    /// Whether it holds the arms of a `match`.
    arms: bool,
    /// Where a line that starts with an operator goes after the line of closers that closes
    /// it: with the operands of the line that holds it.
    operand: usize,
}

/// A token of a line, as far as the rules care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    OpenBrace,
    Open,
    /// A closer of a bracket opened on an earlier line, or of an empty `{}`.
    Close(Kind),
    /// A `}` of a `{` opened on the same line with something in it, as `Foo { a }`, which
    /// ends no statement.
    CloseInline,
    CloseAttribute,
    Semicolon,
    Comma,
    Question,
    /// A `=` alone, not part of `==`, `=>` or `+=`.
    Assign,
    Where,
    Other,
}

impl Token {
    /// Whether a line that ends with it ends its statement or item, or opens a bracket.
    fn ends(self) -> bool {
        matches!(
            self,
            Self::OpenBrace
                | Self::Open
                | Self::Close(Kind::Brace)
                | Self::CloseAttribute
                | Self::Semicolon
                | Self::Comma
        )
    }

    fn closes(self) -> bool {
        matches!(
            self,
            Self::Close(_) | Self::CloseInline | Self::CloseAttribute
        )
    }
}

/// How a line goes on with the statement of the line before, by what it starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Start {
    /// A `.` that is not part of `..`: a call or field of a chain.
    Dot,
    /// An operator then a space: an operand, or a pattern after `|`.
    Operator,
    Other,
}

impl Start {
    fn of(text: &[u8]) -> Self {
        if text.starts_with(b".") && !text.starts_with(b"..") {
            return Self::Dot;
        }
        let operator = text
            .iter()
            .take(2)
            .take_while(|byte| OPERATOR_BYTES.contains(byte))
            .count();
        if operator > 0 && text.get(operator) == Some(&b' ') {
            Self::Operator
        } else {
            Self::Other
        }
    }
}

/// The code line before, and where the lines that go on with its statement go.
#[derive(Clone, Copy, Debug)]
struct Previous {
    ends: bool,
    /// Whether it holds only closers, after which a chain goes on, as `}` then `.map(f)`.
    closers: bool,
    /// Whether it ends with `=`, after which what follows is its value.
    assigns: bool,
    /// The brackets open at its start.
    depth: usize,
    /// Where a line that starts with an operator goes.
    operand: usize,
    /// Where a line that starts with `.` goes.
    chain: usize,
    /// Where any other line goes.
    child: usize,
}

pub(super) struct Rust {
    settings: Settings,
    inside: Inside,
    open: Vec<Open>,
    /// Brackets open past [`MAX_DEPTH`](super::pass::MAX_DEPTH).
    deeper: usize,
    /// Whether a `where` clause is open at top level.
    clause: bool,
    previous: Option<Previous>,
    /// The indentation of the nearest non-blank line before.
    last_indent: usize,
    /// The lines read since the pass started.
    lines: u64,
    /// Where an operator line goes inside the brackets the line being read opens.
    operand: usize,
}

/// What lexing one line found.
#[derive(Default)]
struct Tokens {
    first: Option<Token>,
    last: Option<Token>,
    /// Whether every token is a closer or `?`.
    closers_only: bool,
    /// 1 after `#`, 2 after `#!`: a `[` then opens an attribute.
    hash: u8,
    /// Whether the word `match` came, so that the next `{` holds arms.
    matched: bool,
    /// Where an operator line goes after the bracket closed last, from its opener's line.
    popped: Option<usize>,
    /// Where the last block comment opened on this line opens, outside any other.
    comment_at: Option<usize>,
}

impl Tokens {
    fn push(&mut self, token: Token) {
        if self.first.is_none() {
            self.first = Some(token);
            self.closers_only = true;
        }
        self.closers_only &= token.closes() || token == Token::Question;
        self.last = Some(token);
        self.hash = 0;
    }
}

/// Where a line that starts with `text`, past its indentation, is: `base` past the
/// opener's line of the bracket at hand, in a `match`'s arms, or in a `where` clause.
struct Place<'a> {
    text: &'a [u8],
    base: usize,
    arms: bool,
    clause: bool,
}

impl Place<'_> {
    /// Whether the line takes no continuation: a block's `{`, `where`, `else`, a closer, or
    /// a pattern after `|` in an arm, which sits with the first one as rustfmt writes it.
    fn starts_alone(&self) -> bool {
        let text = self.text;
        text.starts_with(b"{")
            || starts_with_word(text, b"where")
            || starts_with_word(text, b"else")
            || matches!(text.first(), Some(b'}' | b')' | b']'))
            || (self.arms && text.starts_with(b"| "))
    }
}

/// How a line goes on with the statement of the code line before: what it starts with,
/// and that line.
type Continued = Option<(Start, Previous)>;

impl Lexer for Rust {
    fn fresh(settings: Settings) -> Self {
        Self {
            settings,
            inside: Inside::Code,
            open: Vec::new(),
            deeper: 0,
            clause: false,
            previous: None,
            last_indent: 0,
            lines: 0,
            operand: 0,
        }
    }

    fn reset(&mut self, settings: Settings) {
        let mut open = std::mem::take(&mut self.open);
        open.clear();
        *self = Self::fresh(settings);
        self.open = open;
    }

    fn is_anchor(line: &[u8]) -> bool {
        match line.first() {
            Some(b'}') => return true,
            Some(b'#' | b'a'..=b'z') => {}
            _ => return false,
        }
        ITEM_WORDS.iter().any(|word| starts_with_word(line, word))
            || line.starts_with(b"macro_rules!")
            || line.starts_with(b"#[")
            || line.starts_with(b"#!")
    }

    fn is_inside(&self) -> bool {
        self.inside != Inside::Code
    }

    fn line(&mut self, text: &str, indent: usize) {
        if !text.trim_matches([' ', '\t', '\r']).is_empty() {
            self.last_indent = indent;
        }
        let place = self.place(text.trim_start_matches([' ', '\t']).as_bytes());
        let continued = place.and_then(|place| self.continues(&place));
        let depth = self.depth();
        let in_string = matches!(self.inside, Inside::Str | Inside::Raw(_));
        self.operand = self.layout(None, indent, continued).0;
        let tokens = self.lex(text, indent);
        if in_string && let Some(previous) = &mut self.previous {
            // A line a string goes on to lays out what follows as the line the string
            // starts on does.
            self.lines = self.lines.wrapping_add(1);
            if let Some(last) = tokens.last {
                previous.ends = last.ends();
                previous.closers = false;
                previous.assigns = last == Token::Assign;
            }
            return;
        }
        self.end_line(&tokens, indent, continued, depth);
    }

    fn answer(&self, head: &Head, _new: bool) -> Option<Answer> {
        match self.inside {
            Inside::Code => {}
            Inside::Str | Inside::Raw(_) => return Some(Answer::Keep),
            Inside::Comment { column, .. } => {
                let column = match head.first() {
                    Some(b'*') => column.saturating_add(1),
                    _ => self.last_indent,
                };
                return Some(Answer::Column(column));
            }
        }
        if self.deeper > 0 {
            return None;
        }
        if matches!(head.first(), Some(b'}' | b')' | b']')) {
            let top = self.open.last();
            return Some(Answer::Column(top.map_or(0, |open| open.indent)));
        }
        let place = self.place(head.text())?;
        let column = match self.continues(&place) {
            Some((Start::Dot, previous)) => previous.chain,
            Some((Start::Operator, previous)) => previous.operand,
            Some((Start::Other, previous)) => previous.child,
            None if place.clause && !place.starts_alone() => {
                place.base.saturating_add(self.settings.indent_size())
            }
            None => place.base,
        };
        Some(Answer::Column(column))
    }
}

impl Rust {
    fn depth(&self) -> usize {
        self.open.len().saturating_add(self.deeper)
    }

    /// Where a line that starts with `text` is, or none inside a string or comment, or past
    /// the brackets kept.
    fn place<'a>(&self, text: &'a [u8]) -> Option<Place<'a>> {
        if self.inside != Inside::Code || self.deeper > 0 {
            return None;
        }
        let top = self.open.last();
        let size = self.settings.indent_size();
        Some(Place {
            text,
            base: top.map_or(0, |open| open.indent.saturating_add(size)),
            arms: top.is_some_and(|open| open.arms),
            clause: top.map_or(self.clause, |open| open.clause),
        })
    }

    /// How a line at `place` goes on with the statement of the code line before, or none if
    /// it starts a statement, or a list's next item.
    fn continues(&self, place: &Place<'_>) -> Continued {
        if place.starts_alone() || place.clause {
            return None;
        }
        let start = Start::of(place.text);
        let previous = self.previous?;
        if previous.ends && !(previous.closers && start == Start::Dot) {
            return None;
        }
        if self.depth() > previous.depth {
            // Inside a bracket the line before opened, after what followed its opener
            // there: the bracket's first line.
            let base = place.base;
            let first = Previous {
                ends: false,
                closers: false,
                assigns: false,
                depth: self.depth(),
                operand: base,
                chain: base,
                child: base,
            };
            return Some((start, first));
        }
        Some((start, previous))
    }

    /// Where the lines after a line indented to `indent` go: those that start with an
    /// operator, those that start with `.`, and the others. `tokens` are the line's, once it
    /// is read.
    fn layout(
        &self,
        tokens: Option<&Tokens>,
        indent: usize,
        continued: Continued,
    ) -> (usize, usize, usize) {
        let child = indent.saturating_add(self.settings.indent_size());
        if let Some(tokens) = tokens.filter(|tokens| tokens.first.is_some_and(Token::closes)) {
            // A line of closers ends a call or block the statement goes on after, as `})`
            // then `.await`; a line that only starts with closers starts a statement.
            return if tokens.closers_only {
                (tokens.popped.unwrap_or(child), indent, child)
            } else {
                (child, child, child)
            };
        }
        match continued {
            Some((Start::Dot, previous)) => (previous.operand, indent, child),
            Some((Start::Operator, _)) => (indent, child, child),
            // A line that goes on with a statement has the next such line beside it: the
            // continuation does not stack.
            Some((Start::Other, previous)) if previous.assigns => (previous.operand, child, indent),
            Some((Start::Other, _)) => (child, child, indent),
            None => (child, child, child),
        }
    }

    fn end_line(&mut self, tokens: &Tokens, indent: usize, continued: Continued, depth: usize) {
        self.lines = self.lines.wrapping_add(1);
        let Some(last) = tokens.last else {
            return;
        };
        let (operand, chain, child) = self.layout(Some(tokens), indent, continued);
        self.previous = Some(Previous {
            ends: last.ends(),
            closers: tokens.closers_only,
            assigns: last == Token::Assign,
            depth,
            operand,
            chain,
            child,
        });
    }

    fn set_clause(&mut self) {
        match self.open.last_mut() {
            Some(open) => open.clause = true,
            None => self.clause = true,
        }
    }

    fn end_clause(&mut self) {
        match self.open.last_mut() {
            Some(open) => open.clause = false,
            None => self.clause = false,
        }
    }
}
