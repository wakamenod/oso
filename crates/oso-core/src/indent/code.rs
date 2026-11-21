//! The indentation rules of the languages of brackets and tags (ADR 0164): CSS, JSON,
//! JavaScript and TypeScript (JSX among them), Go and HTML, over one lexer that each reads in
//! its own dialect (`scan`).
//!
//! Must not depend on: `buffer`, or the tree highlighting parses; it lexes the text itself.
//!
//! - Inside a bracket, or an element, a line goes one level past the line that opened it, and
//!   a line that starts with its closer (`}`, `)`, `]`, `</`, or a start tag's `>` or `/>`)
//!   back to that line. At top level a line goes to column 0.
//! - Inside a multi-line string (a template literal, a raw string, a quoted attribute) or the
//!   text of `<pre>` or `<textarea>`, a line keeps its indentation. Inside a block comment, a
//!   line that starts with `*` goes one past the comment's opening column, and any other to the
//!   indentation of the line before.
//! - JavaScript and Go: a line that goes on with the statement of the line before (it ended
//!   with an operator, or this one starts with `.`, `?`, `:`, `&&`, `||` or `??`) goes one level
//!   past the statement's first line; the continuation does not stack. A `case` or `default`
//!   label sits one level in from its `switch` in JavaScript, the lines after it one more, as
//!   Prettier writes; in Go a label sits with its `switch`, as gofmt writes.
//! - HTML: a void element, a `/>` and a comment open nothing; `<script>`'s text is read as
//!   JavaScript and `<style>`'s as CSS, one level in from their tag's line.

mod scan;

use std::marker::PhantomData;

use super::pass::{Head, Lexer, starts_with_word};
use super::{Answer, Settings};

/// What a lexer reads its text as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dialect {
    Css,
    Json,
    /// JavaScript or TypeScript, with JSX's elements when `jsx`.
    Script {
        jsx: bool,
    },
    Go,
    Html,
}

/// A language: its dialect and the lines that are anchors in it, at column 0.
pub(super) trait Flavor {
    const DIALECT: Dialect;

    fn is_anchor(line: &[u8]) -> bool;
}

/// What the lexer is inside at a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Inside {
    Code,
    /// A block comment, `/* */` or HTML's `<!-- -->`, opened at `column`.
    Comment {
        column: usize,
        html: bool,
    },
    /// A string that goes on past its line, closed by `quote`: a template literal, a Go raw
    /// string, a quoted attribute.
    Quoted(u8),
    /// The text of `<pre>` or `<textarea>`, whose white space is its own.
    Verbatim,
}

/// What an element's text is, by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Element {
    Normal,
    /// An HTML element with no end tag, as `<br>`.
    Void,
    /// `<script>`, read as JavaScript.
    Script,
    /// `<style>`, read as CSS.
    Style,
    /// `<pre>` or `<textarea>`.
    Verbatim,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Brace,
    Paren,
    Bracket,
    /// The `${` of a template literal: its `}` goes back into the literal.
    Template,
    /// A start tag whose `>` is still to come.
    StartTag(Element),
    /// An element's content.
    Tag(Element),
}

impl Kind {
    fn is_bracket(self) -> bool {
        matches!(
            self,
            Self::Brace | Self::Paren | Self::Bracket | Self::Template
        )
    }
}

/// The most bytes of a tag's name kept, to match an end tag to its element.
const NAME_BYTES: usize = 16;

/// A tag's name, as far as [`NAME_BYTES`]; in HTML, lowercased.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Name {
    bytes: [u8; NAME_BYTES],
    len: usize,
}

impl Name {
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// A bracket or element still open.
#[derive(Clone, Copy, Debug)]
struct Open {
    kind: Kind,
    /// The indentation of the line that holds it.
    indent: usize,
    /// The indentation of the first line of the statement at hand directly inside it.
    statement: usize,
    /// Whether a `case` label came directly inside it: it is a `switch`'s body.
    cases: bool,
    name: Name,
}

/// The operators a JavaScript line that goes on with a statement starts with, a space after:
/// a ternary's branches, a condition's operands, a union type's members.
const LEADING_OPERATORS: [&[u8]; 6] = [b"&&", b"||", b"??", b"?", b":", b"|"];

/// The rules of one language.
pub(super) struct Code<F> {
    settings: Settings,
    /// What the text is read as here: the language's own, or in HTML, a script's or style's.
    dialect: Dialect,
    inside: Inside,
    open: Vec<Open>,
    /// Brackets and elements open past [`MAX_DEPTH`](super::pass::MAX_DEPTH).
    deeper: usize,
    /// The indentation of the first line of the statement at hand at top level.
    statement: usize,
    /// The indentation of the nearest non-blank line before.
    last_indent: usize,
    /// The indentation a bracket opened on the line at hand counts from: the line's own, or
    /// in Go, on a line that goes on with a statement, the statement's.
    opens_at: usize,
    /// The code line before: the depth it ended at and whether it ended with an operator.
    previous: Option<(usize, bool)>,
    /// What came last, for telling a `/` that starts a regular expression, and a `<` that
    /// starts a JSX element, from an operator.
    before: Before,
    flavor: PhantomData<F>,
}

/// What came last in code: nothing yet, an operand after which `/` divides, or an operator
/// after which an operand starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Before {
    Start,
    Operand,
    Operator,
}

impl<F: Flavor> Lexer for Code<F> {
    fn fresh(settings: Settings) -> Self {
        Self {
            settings,
            dialect: F::DIALECT,
            inside: Inside::Code,
            open: Vec::new(),
            deeper: 0,
            statement: 0,
            last_indent: 0,
            opens_at: 0,
            previous: None,
            before: Before::Start,
            flavor: PhantomData,
        }
    }

    fn reset(&mut self, settings: Settings) {
        let mut open = std::mem::take(&mut self.open);
        open.clear();
        *self = Self::fresh(settings);
        self.open = open;
    }

    fn is_anchor(line: &[u8]) -> bool {
        F::is_anchor(line)
    }

    fn relative(settings: Settings, line: &[u8], column: usize, head: &[u8]) -> usize {
        let size = settings.indent_size();
        let mut column = column;
        if opens(F::DIALECT, line) {
            column = column.saturating_add(size);
        }
        if closes(F::DIALECT, head) {
            column = column.saturating_sub(size);
        }
        column
    }

    fn is_inside(&self) -> bool {
        self.inside != Inside::Code
    }

    fn line(&mut self, text: &str, indent: usize) {
        let bytes = text.as_bytes();
        let start = bytes
            .iter()
            .position(|byte| !matches!(byte, b' ' | b'\t' | b'\r'));
        let Some(start) = start else {
            return;
        };
        self.last_indent = indent;
        self.opens_at = indent;
        let head = &bytes[start..];
        if self.inside == Inside::Code && self.deeper == 0 && self.in_code() {
            let label = self.is_label(head);
            if label && let Some(open) = self.open.last_mut() {
                open.cases = true;
            }
            // A line that starts a statement, not one that goes on with one or closes a
            // bracket, sets where the statement's next lines go on from.
            let closes = matches!(head.first(), Some(b'}' | b')' | b']'));
            let continues = self.continues(head);
            if !label && !closes && !continues {
                self.set_statement(indent);
            }
            // gofmt puts a block opened on a condition's next line, as `if a &&` then `b {`,
            // with the statement, not with that line.
            if continues && self.dialect == Dialect::Go {
                self.opens_at = self.statement_here();
            }
        }
        if self.inside == Inside::Code {
            self.before = Before::Start;
        }
        let operator = self.lex(bytes, indent);
        if self.inside == Inside::Code && self.in_code() {
            self.previous = Some((self.depth(), operator && !self.is_label(head)));
        } else {
            self.previous = None;
        }
    }

    fn answer(&self, head: &Head, _new: bool) -> Option<Answer> {
        match self.inside {
            Inside::Code => {}
            Inside::Quoted(_) | Inside::Verbatim => return Some(Answer::Keep),
            Inside::Comment { column, html } => {
                let column = match head.first() {
                    Some(b'*') if !html => column.saturating_add(1),
                    _ => self.last_indent,
                };
                return Some(Answer::Column(column));
            }
        }
        if self.deeper > 0 {
            return None;
        }
        let text = head.text();
        let size = self.settings.indent_size();
        let top = self.open.last();
        if self.in_code() && matches!(text.first(), Some(b'}' | b')' | b']')) {
            return Some(Answer::Column(top.map_or(0, |open| open.indent)));
        }
        if self.has_tags() && text.starts_with(b"</") {
            // The element the end tag names, which closes what is open inside it; or the
            // innermost, for a name the line's start does not hold whole.
            let name = tag_name(&text[2..], F::DIALECT == Dialect::Html);
            let tags = || {
                self.open
                    .iter()
                    .rev()
                    .filter(|open| matches!(open.kind, Kind::Tag(_)))
            };
            let element = tags()
                .find(|open| open.name == name)
                .or_else(|| tags().next());
            return Some(Answer::Column(element.map_or(0, |open| open.indent)));
        }
        if let Some(open) = top.filter(|open| matches!(open.kind, Kind::StartTag(_)))
            && (text.starts_with(b">") || text.starts_with(b"/>"))
        {
            return Some(Answer::Column(open.indent));
        }
        let base = top.map_or(0, |open| open.indent.saturating_add(size));
        if !self.in_code() {
            return Some(Answer::Column(base));
        }
        let column = if self.is_label(text) {
            match self.dialect {
                Dialect::Go => top.map_or(0, |open| open.indent),
                _ => base,
            }
        } else if self.continues(text) {
            self.statement_here().saturating_add(size)
        } else if top.is_some_and(|open| open.cases) && self.dialect != Dialect::Go {
            base.saturating_add(size)
        } else {
            base
        };
        Some(Answer::Column(column))
    }
}

impl<F: Flavor> Code<F> {
    fn depth(&self) -> usize {
        self.open.len().saturating_add(self.deeper)
    }

    /// Whether a line here starts in code, not in an element's text or a start tag.
    fn in_code(&self) -> bool {
        match self.open.last().map(|open| open.kind) {
            Some(Kind::Brace | Kind::Paren | Kind::Bracket | Kind::Template) => true,
            // Until its end tag, which is read as HTML.
            Some(Kind::Tag(Element::Script | Element::Style)) => self.dialect != Dialect::Html,
            Some(Kind::Tag(_) | Kind::StartTag(_)) => false,
            None => self.dialect != Dialect::Html,
        }
    }

    /// Whether the language has elements: HTML, or JavaScript with JSX.
    fn has_tags(&self) -> bool {
        matches!(F::DIALECT, Dialect::Html | Dialect::Script { jsx: true })
    }

    /// Whether a line that starts with `text` is a `case` or `default` label of a `switch`.
    fn is_label(&self, text: &[u8]) -> bool {
        if !matches!(self.dialect, Dialect::Script { .. } | Dialect::Go) {
            return false;
        }
        let brace = self
            .open
            .last()
            .is_some_and(|open| open.kind == Kind::Brace);
        if !brace {
            return false;
        }
        if starts_with_word(text, b"case") {
            return true;
        }
        // `default:` is a label in Go always, and in JavaScript in a `switch`, not as an
        // object's key.
        if !starts_with_word(text, b"default") {
            return false;
        }
        let after = text.get(b"default".len()..).unwrap_or_default();
        let colon = after.iter().find(|byte| !matches!(byte, b' ' | b'\t'));
        let switch = self.open.last().is_some_and(|open| open.cases);
        colon == Some(&b':') && (self.dialect == Dialect::Go || switch)
    }

    /// Whether a line that starts with `text` goes on with the statement of the code line
    /// before: that line ended with an operator at this depth, or this one starts with one.
    fn continues(&self, text: &[u8]) -> bool {
        if !matches!(self.dialect, Dialect::Script { .. } | Dialect::Go) || self.is_label(text) {
            return false;
        }
        if self
            .previous
            .is_some_and(|(depth, operator)| operator && depth == self.depth())
        {
            return true;
        }
        let Dialect::Script { .. } = self.dialect else {
            return false;
        };
        let space_after = |len: usize| matches!(text.get(len), Some(b' ' | b'\t') | None);
        let chain =
            (text.starts_with(b".") && !text.starts_with(b"...")) || text.starts_with(b"?.");
        let operator = LEADING_OPERATORS
            .iter()
            .any(|operator| text.starts_with(operator) && space_after(operator.len()));
        chain || operator
    }

    /// The indentation of the first line of the statement at hand here.
    fn statement_here(&self) -> usize {
        self.open
            .last()
            .map_or(self.statement, |open| open.statement)
    }

    fn set_statement(&mut self, indent: usize) {
        match self.open.last_mut() {
            Some(open) => open.statement = indent,
            None => self.statement = indent,
        }
    }
}

/// CSS: anchors are the rules and at-rules that start at column 0.
pub(super) struct Css;

impl Flavor for Css {
    const DIALECT: Dialect = Dialect::Css;

    fn is_anchor(line: &[u8]) -> bool {
        matches!(
            line.first(),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'.' | b'#' | b'@' | b':' | b'[' | b'}' | b'&')
        )
    }
}

/// JSON: the document's own bracket at column 0 is its only anchor.
pub(super) struct Json;

impl Flavor for Json {
    const DIALECT: Dialect = Dialect::Json;

    fn is_anchor(line: &[u8]) -> bool {
        matches!(line.first(), Some(b'{' | b'['))
    }
}

/// The words a JavaScript or TypeScript line starts with at column 0 that make it an anchor,
/// beside `@` and `}`.
const SCRIPT_WORDS: [&[u8]; 14] = [
    b"import",
    b"export",
    b"function",
    b"class",
    b"const",
    b"let",
    b"var",
    b"interface",
    b"type",
    b"enum",
    b"declare",
    b"namespace",
    b"async",
    b"abstract",
];

fn is_script_anchor(line: &[u8]) -> bool {
    matches!(line.first(), Some(b'}' | b'@'))
        || SCRIPT_WORDS.iter().any(|word| starts_with_word(line, word))
}

/// TypeScript, with no JSX.
pub(super) struct TypeScript;

impl Flavor for TypeScript {
    const DIALECT: Dialect = Dialect::Script { jsx: false };

    fn is_anchor(line: &[u8]) -> bool {
        is_script_anchor(line)
    }
}

/// JavaScript and TSX, with JSX.
pub(super) struct Jsx;

impl Flavor for Jsx {
    const DIALECT: Dialect = Dialect::Script { jsx: true };

    fn is_anchor(line: &[u8]) -> bool {
        is_script_anchor(line)
    }
}

/// The words a Go line starts with at column 0 that make it an anchor, beside `}` and `)`.
const GO_WORDS: [&[u8]; 6] = [b"func", b"type", b"var", b"const", b"import", b"package"];

/// Go.
pub(super) struct Go;

impl Flavor for Go {
    const DIALECT: Dialect = Dialect::Go;

    fn is_anchor(line: &[u8]) -> bool {
        matches!(line.first(), Some(b'}' | b')'))
            || GO_WORDS.iter().any(|word| starts_with_word(line, word))
    }
}

/// HTML: an element's tag, an end tag or a doctype at column 0 is an anchor.
pub(super) struct Html;

impl Flavor for Html {
    const DIALECT: Dialect = Dialect::Html;

    fn is_anchor(line: &[u8]) -> bool {
        line.first() == Some(&b'<')
            && matches!(line.get(1), Some(b'a'..=b'z' | b'A'..=b'Z' | b'/' | b'!'))
    }
}

/// Whether a line whose text is `line`, past its indentation, opens a level for the next:
/// it ends with an opening bracket, or in a language of elements, it is a start tag that
/// stays open. For the answer past the budget, which reads only the line before.
pub(super) fn opens(dialect: Dialect, line: &[u8]) -> bool {
    let line = line.trim_ascii_end();
    if matches!(line.last(), Some(b'{' | b'[' | b'(')) {
        return true;
    }
    let tags = matches!(dialect, Dialect::Html | Dialect::Script { jsx: true });
    tags && line.starts_with(b"<")
        && line.ends_with(b">")
        && matches!(line.get(1), Some(b'a'..=b'z' | b'A'..=b'Z'))
        && !line.ends_with(b"/>")
        && !line.windows(2).any(|pair| pair == b"</")
        && !(dialect == Dialect::Html && scan::is_void(tag_name(&line[1..], true).as_bytes()))
}

/// Whether a line that starts with `head` closes a level: a closing bracket, or an end tag.
pub(super) fn closes(dialect: Dialect, head: &[u8]) -> bool {
    let tags = matches!(dialect, Dialect::Html | Dialect::Script { jsx: true });
    matches!(head.first(), Some(b'}' | b']' | b')')) || (tags && head.starts_with(b"</"))
}

/// The name `text` starts with, lowercased in HTML, which compares names so.
fn tag_name(text: &[u8], html: bool) -> Name {
    let mut name = Name::default();
    for &byte in text.iter().take_while(|&&byte| scan::is_name_byte(byte)) {
        if name.len == NAME_BYTES {
            break;
        }
        name.bytes[name.len] = if html {
            byte.to_ascii_lowercase()
        } else {
            byte
        };
        name.len += 1;
    }
    name
}
