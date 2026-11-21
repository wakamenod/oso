//! Reading one line in a dialect (ADR 0164): the brackets it opens and closes, its strings,
//! comments, template literals and regular expressions, and in HTML and JSX, its elements.
//!
//! Must not depend on: `buffer`; it reads bytes. Every loop moves forward, so a line costs
//! its length, twice at most: a regular expression that does not close on its line is looked
//! for once, and the line's later `/`s divide; an end tag looks for its element among the
//! innermost [`TAG_SEARCH`] only, and not at all when no element of its name is open.
//!
//! - A `/` starts a regular expression where an operand can start (after an operator, a `(`,
//!   a keyword such as `return`, or at a statement's start) and the expression closes on its
//!   line; anywhere else, and when it does not close, it divides. JSX's `<` is told the same way,
//!   but for a TSX generic's `<T,>` or `<T extends U>`, which opens no element.
//! - `</script` and `</style` end an HTML page's script or style wherever they come, as a
//!   browser reads them, strings and comments of the script too.

use super::super::pass::{MAX_DEPTH, is_word_byte, starts_with_word};
use super::{Before, Dialect, Element, Inside, Kind, Language, Name, Nesting, Open};
use super::{is_void, tag_name_len};

/// How many of the innermost brackets and elements an end tag looks among for its element:
/// past them, it closes nothing.
const TAG_SEARCH: usize = 64;

/// Whether `word` is one after which a `/` or `<` starts an operand, as after an operator.
fn operand_follows(word: &[u8]) -> bool {
    matches!(
        word,
        b"return"
            | b"typeof"
            | b"case"
            | b"in"
            | b"of"
            | b"new"
            | b"delete"
            | b"void"
            | b"instanceof"
            | b"yield"
            | b"await"
    )
}

/// Where the first of `needle`, which starts with `<`, compared ignoring ASCII case, starts in
/// `bytes` from `from`.
fn find_ignoring_case(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let rest = bytes.get(from..)?;
    rest.iter()
        .enumerate()
        .filter(|&(_, &byte)| byte == b'<')
        .find(|&(at, _)| {
            rest.get(at..at + needle.len())
                .is_some_and(|window| window.eq_ignore_ascii_case(needle))
        })
        .map(|(at, _)| from + at)
}

impl<F: Language> Nesting<F> {
    /// Reads a line, `bytes` without its line break, indented to `indent`, and returns
    /// whether its last token is an operator a statement goes on after.
    pub(super) fn lex(&mut self, bytes: &[u8], indent: usize) -> bool {
        let lead = bytes
            .iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        let mut operator = false;
        let mut at = 0;
        // Where the script or style the line is in ends, if it does on this line.
        let mut raw_end = self.raw_end(bytes, 0);
        self.no_regex = false;
        while at < bytes.len() {
            if raw_end == Some(at) {
                self.end_raw();
                raw_end = None;
            }
            let limit = raw_end.unwrap_or(bytes.len());
            let line = &bytes[..limit];
            let column = indent.saturating_add(at.saturating_sub(lead));
            let was = self.dialect;
            at = match self.inside {
                Inside::Comment { html, .. } => comment_end(line, at, html).map_or(limit, |end| {
                    self.inside = Inside::Code;
                    end
                }),
                Inside::Quoted(quote) => self.quoted(line, at, quote, indent),
                Inside::Verbatim => self.verbatim(line, at),
                Inside::Code => match self.open.last().map(|open| open.kind) {
                    Some(Kind::StartTag(_)) => self.start_tag(line, at),
                    _ if !self.in_code() => self.text(line, at, (indent, column)),
                    _ => self.code(line, at, (indent, column), &mut operator),
                },
            };
            if self.dialect != was && raw_end.is_none() {
                raw_end = self.raw_end(bytes, at);
            }
        }
        if raw_end == Some(bytes.len()) {
            self.end_raw();
        }
        operator
    }

    /// Where `</script` or `</style` ends the script or style the lexer is in, from `from`.
    fn raw_end(&self, bytes: &[u8], from: usize) -> Option<usize> {
        if F::DIALECT != Dialect::Html {
            return None;
        }
        match self.dialect {
            Dialect::Script { .. } => find_ignoring_case(bytes, from, b"</script"),
            Dialect::Css => find_ignoring_case(bytes, from, b"</style"),
            _ => None,
        }
    }

    /// Ends a script or style: what it left open closes, and its end tag is read as HTML.
    fn end_raw(&mut self) {
        let raw = self
            .open
            .iter()
            .rposition(|open| matches!(open.kind, Kind::Tag(Element::Script | Element::Style)));
        if let Some(raw) = raw {
            self.open.truncate(raw + 1);
        }
        self.deeper = 0;
        self.dialect = Dialect::Html;
        self.inside = Inside::Code;
    }

    fn push(&mut self, kind: Kind, indent: usize, name: Name) {
        if self.deeper > 0 || self.open.len() == MAX_DEPTH {
            self.deeper = self.deeper.saturating_add(1);
            return;
        }
        let below = self.open.last().map_or(0, |open| open.names);
        let names = match kind {
            Kind::StartTag(_) | Kind::Tag(_) => below | name.bit(),
            _ => below,
        };
        self.open.push(Open {
            kind,
            indent,
            statement: indent,
            cases: false,
            name,
            names,
        });
    }

    /// Closes the innermost bracket, or one counted past the depth kept. A closer with no
    /// bracket open in its code does nothing.
    fn pop_bracket(&mut self) {
        if self.deeper > 0 {
            self.deeper -= 1;
        } else if self.open.last().is_some_and(|open| open.kind.is_bracket()) {
            self.open.pop();
        }
    }

    /// Reads code from `at`, one token.
    fn code(
        &mut self,
        line: &[u8],
        at: usize,
        (indent, column): (usize, usize),
        operator: &mut bool,
    ) -> usize {
        let byte = line[at];
        let next = line.get(at + 1).copied();
        let script = matches!(self.dialect, Dialect::Script { .. });
        match byte {
            b' ' | b'\t' | b'\r' => return at + 1,
            b'/' if next == Some(b'/') && self.dialect != Dialect::Css => return line.len(),
            b'/' if next == Some(b'*') => {
                self.inside = Inside::Comment {
                    column,
                    html: false,
                };
                return at + 2;
            }
            b'/' if script && self.before != Before::Operand && !self.no_regex => {
                if let Some(end) = regex_end(line, at + 1) {
                    self.before = Before::Operand;
                    *operator = false;
                    return end;
                }
                self.no_regex = true;
            }
            _ => {}
        }
        *operator = false;
        match byte {
            b'"' | b'\'' => {
                self.before = Before::Operand;
                return string_end(line, at + 1, byte);
            }
            b'`' if script || self.dialect == Dialect::Go => {
                self.before = Before::Operand;
                self.inside = Inside::Quoted(b'`');
                return at + 1;
            }
            b'{' | b'(' | b'[' | b'}' | b')' | b']' => {
                self.bracket(byte);
                return at + 1;
            }
            b'<' if self.dialect == (Dialect::Script { jsx: true })
                && self.before != Before::Operand
                && next.is_some_and(|next| next == b'>' || next.is_ascii_alphabetic())
                && !is_generic(&line[at + 1..]) =>
            {
                return self.open_tag(line, at, indent);
            }
            _ => {}
        }
        if is_word_byte(byte) || byte == b'$' {
            return self.word(line, at);
        }
        let pair = &line[at..line.len().min(at + 2)];
        if pair == b"++" || pair == b"--" {
            self.before = Before::Operand;
            return at + 2;
        }
        // An operator, or punctuation a statement does not go on after.
        self.before = Before::Operator;
        match byte {
            b';' => self.before = Before::Start,
            b',' | b'<' | b'>' | b'#' | b'@' => {}
            _ => *operator = true,
        }
        at + 1
    }

    /// Opens or closes a bracket at `byte`; a `}` that closes a template literal's `${` goes
    /// back into the literal.
    fn bracket(&mut self, byte: u8) {
        let kind = match byte {
            b'{' => Kind::Brace,
            b'(' => Kind::Paren,
            b'[' => Kind::Bracket,
            _ => {
                let top = self.open.last().map(|open| open.kind);
                let template = self.deeper == 0 && byte == b'}' && top == Some(Kind::Template);
                self.pop_bracket();
                if template {
                    self.inside = Inside::Quoted(b'`');
                }
                self.before = Before::Operand;
                return;
            }
        };
        self.push(kind, self.opens_at, Name::default());
        self.before = Before::Start;
    }

    /// Reads a word from `at`: a keyword such as `return` lets an operand follow, as an
    /// operator does; any other word is an operand.
    fn word(&mut self, line: &[u8], at: usize) -> usize {
        let end = at
            + line[at..]
                .iter()
                .take_while(|&&byte| is_word_byte(byte) || byte == b'$')
                .count();
        let word = &line[at..end];
        self.before = if operand_follows(word) {
            Before::Operator
        } else {
            Before::Operand
        };
        end
    }

    /// Reads a quoted string that goes on past its line, from `at`, as far as its end or the
    /// line's.
    fn quoted(&mut self, line: &[u8], mut at: usize, quote: u8, indent: usize) -> usize {
        let script = matches!(self.dialect, Dialect::Script { .. });
        let template = quote == b'`' && script;
        while let Some(&byte) = line.get(at) {
            match byte {
                b'\\' if template => at += 2,
                _ if byte == quote => {
                    self.inside = Inside::Code;
                    return at + 1;
                }
                b'$' if template && line.get(at + 1) == Some(&b'{') => {
                    self.inside = Inside::Code;
                    self.push(Kind::Template, indent, Name::default());
                    self.before = Before::Start;
                    return at + 2;
                }
                _ => at += 1,
            }
        }
        line.len()
    }

    /// Reads the text of `<pre>` or `<textarea>` up to its end tag, which is then read as an
    /// end tag.
    fn verbatim(&mut self, line: &[u8], at: usize) -> usize {
        let Some(open) = self.open.last() else {
            self.inside = Inside::Code;
            return at;
        };
        let name = open.name;
        let mut from = at;
        while let Some(end) = find_ignoring_case(line, from, b"</") {
            let rest = &line[end + 2..];
            if rest.len() >= name.len && rest[..name.len].eq_ignore_ascii_case(name.as_bytes()) {
                self.inside = Inside::Code;
                return end;
            }
            from = end + 2;
        }
        line.len()
    }

    /// Reads an element's text, or HTML outside any element, from `at`.
    fn text(&mut self, line: &[u8], at: usize, (indent, column): (usize, usize)) -> usize {
        let html = self.dialect == Dialect::Html;
        match line[at] {
            b'<' if html && line[at..].starts_with(b"<!--") => {
                self.inside = Inside::Comment { column, html: true };
                at + 4
            }
            b'<' if html && matches!(line.get(at + 1), Some(b'!' | b'?')) => line[at..]
                .iter()
                .position(|&byte| byte == b'>')
                .map_or(line.len(), |end| at + end + 1),
            b'<' if line.get(at + 1) == Some(&b'/') => self.close_tag(line, at + 2),
            b'<' if line
                .get(at + 1)
                .is_some_and(|&next| next.is_ascii_alphabetic() || (!html && next == b'>')) =>
            {
                self.open_tag(line, at, indent)
            }
            b'{' if !html => {
                self.push(Kind::Brace, indent, Name::default());
                self.before = Before::Start;
                at + 1
            }
            _ => at + 1,
        }
    }

    /// Opens the element whose start tag starts at `at`, its `<`; a fragment's `<>` opens its
    /// content at once.
    fn open_tag(&mut self, line: &[u8], at: usize, indent: usize) -> usize {
        if line.get(at + 1) == Some(&b'>') {
            self.push(Kind::Tag(Element::Normal), indent, Name::default());
            return at + 2;
        }
        let (name, end) = self.name(line, at + 1);
        let element = if self.dialect == Dialect::Html {
            match name.as_bytes() {
                b"script" => Element::Script,
                b"style" => Element::Style,
                b"pre" | b"textarea" => Element::Verbatim,
                name if is_void(name) => Element::Void,
                _ => Element::Normal,
            }
        } else {
            Element::Normal
        };
        self.push(Kind::StartTag(element), indent, name);
        end
    }

    /// The name of a tag that starts at `at`, in HTML lowercased, and where it ends.
    fn name(&self, line: &[u8], at: usize) -> (Name, usize) {
        let (name, len) = tag_name_len(&line[at..], self.dialect == Dialect::Html);
        (name, at + len)
    }

    /// Reads an end tag whose name starts at `at`: it closes the innermost element of that
    /// name, and what is open inside it. One that closes nothing open does nothing.
    fn close_tag(&mut self, line: &[u8], at: usize) -> usize {
        let (name, end) = self.name(line, at);
        let end = line[end..]
            .iter()
            .position(|&byte| byte == b'>')
            .map_or(line.len(), |close| end + close + 1);
        if self.deeper > 0 {
            self.deeper -= 1;
            return end;
        }
        let open = self
            .open
            .last()
            .is_some_and(|open| open.names & name.bit() != 0);
        if !open {
            return end;
        }
        let from = self.open.len().saturating_sub(TAG_SEARCH);
        let element = self.open[from..]
            .iter()
            .rposition(|open| matches!(open.kind, Kind::Tag(_)) && open.name == name);
        if let Some(element) = element {
            self.open.truncate(from + element);
        }
        end
    }

    /// Reads a start tag's attributes from `at`, as far as its `>` or `/>`.
    fn start_tag(&mut self, line: &[u8], at: usize) -> usize {
        match line[at] {
            quote @ (b'"' | b'\'') => {
                self.inside = Inside::Quoted(quote);
                at + 1
            }
            b'{' if self.dialect != Dialect::Html => {
                let indent = self.open.last().map_or(0, |open| open.indent);
                self.push(Kind::Brace, indent, Name::default());
                self.before = Before::Start;
                at + 1
            }
            b'/' if line.get(at + 1) == Some(&b'>') => {
                self.open.pop();
                self.before = Before::Operand;
                at + 2
            }
            b'>' => {
                self.content();
                at + 1
            }
            _ => at + 1,
        }
    }

    /// A start tag's `>`: its element's content starts, as the element says.
    fn content(&mut self) {
        let Some(open) = self.open.last_mut() else {
            return;
        };
        let Kind::StartTag(element) = open.kind else {
            return;
        };
        open.kind = Kind::Tag(element);
        match element {
            Element::Void => {
                self.open.pop();
            }
            Element::Script => {
                self.dialect = Dialect::Script { jsx: false };
                self.before = Before::Start;
            }
            Element::Style => self.dialect = Dialect::Css,
            Element::Verbatim => self.inside = Inside::Verbatim,
            Element::Normal => {}
        }
    }
}

/// Whether the text after a `<` in TSX is a generic's type parameters, `<T,>` or `<T extends
/// U>`, as an arrow function's: a name, then a `,` or the word `extends`.
fn is_generic(after: &[u8]) -> bool {
    let name = after.iter().take_while(|&&byte| is_word_byte(byte)).count();
    let rest = after[name..].trim_ascii_start();
    rest.starts_with(b",") || starts_with_word(rest, b"extends")
}

/// Where a block comment that goes on at `at` ends, past its closer, if it does on the line.
fn comment_end(line: &[u8], at: usize, html: bool) -> Option<usize> {
    let closer: &[u8] = if html { b"-->" } else { b"*/" };
    line.get(at..)?
        .windows(closer.len())
        .position(|window| window == closer)
        .map(|end| at + end + closer.len())
}

/// Where a one-line string whose text starts at `at` ends, past its closing `quote`, or the
/// line's end.
fn string_end(line: &[u8], mut at: usize, quote: u8) -> usize {
    while let Some(&byte) = line.get(at) {
        match byte {
            b'\\' => at += 2,
            _ if byte == quote => return at + 1,
            _ => at += 1,
        }
    }
    line.len()
}

/// Where a regular expression whose pattern starts at `at` ends, past its flags, if it closes
/// on the line: a `/` outside a class (`[...]`) and not escaped.
fn regex_end(line: &[u8], mut at: usize) -> Option<usize> {
    let mut class = false;
    while let Some(&byte) = line.get(at) {
        match byte {
            b'\\' => at += 2,
            b'[' => {
                class = true;
                at += 1;
            }
            b']' => {
                class = false;
                at += 1;
            }
            b'/' if !class => {
                at += 1;
                while line.get(at).is_some_and(u8::is_ascii_alphabetic) {
                    at += 1;
                }
                return Some(at);
            }
            _ => at += 1,
        }
    }
    None
}
