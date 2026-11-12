//! The lexer of Rust's rules (ADR 0115): what each byte of a line opens or closes, from
//! comments and strings to brackets, and the tokens the rules read at a line's ends.
//!
//! Must not depend on: `buffer`, or the tree highlighting parses; it lexes the text itself.

use super::super::pass::{MAX_DEPTH, is_word_byte, width};
use super::{Inside, Kind, Open, Rust, Token, Tokens};

/// The longest char literal after its opening quote and backslash: `u{10FFFF}'`.
const MAX_CHAR: usize = 11;

impl Rust {
    /// Lexes one line, `indent` its indentation as the command leaves it, and returns the
    /// tokens the rules read.
    pub(super) fn lex(&mut self, text: &str, indent: usize) -> Tokens {
        let mut tokens = Tokens::default();
        if matches!(self.inside, Inside::Str | Inside::Raw(_)) && !text.is_empty() {
            tokens.push(Token::Other);
        }
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if self.inside == Inside::Code {
                // Spaces between tokens, the indentation first, take no token.
                index += bytes[index..]
                    .iter()
                    .take_while(|&&byte| matches!(byte, b' ' | b'\t' | b'\r'))
                    .count();
                if index == bytes.len() {
                    break;
                }
            }
            index = match self.inside {
                Inside::Code => match self.code(text, index, indent, &mut tokens) {
                    Some(next) => next,
                    None => break,
                },
                Inside::Str => self.string(bytes, index),
                Inside::Raw(hashes) => self.raw(bytes, index, hashes),
                Inside::Comment { depth, column } => self.comment(bytes, index, depth, column),
            };
        }
        if let (Inside::Comment { depth, .. }, Some(at)) = (self.inside, tokens.comment_at) {
            let lead = text.len() - text.trim_start_matches([' ', '\t']).len();
            let column = width(&text[lead.min(at)..at], indent, self.settings.tab_width());
            self.inside = Inside::Comment { depth, column };
        }
        tokens
    }

    /// Lexes the code at `index` and returns where it goes on, or none at a line comment.
    fn code(
        &mut self,
        text: &str,
        index: usize,
        indent: usize,
        tokens: &mut Tokens,
    ) -> Option<usize> {
        let bytes = text.as_bytes();
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        let after = match byte {
            b'/' if next == Some(b'/') => return None,
            b'/' if next == Some(b'*') => {
                // Its column is measured once, at the line's end, if it is still open there.
                self.inside = Inside::Comment {
                    depth: 1,
                    column: indent,
                };
                tokens.comment_at = Some(index);
                index + 2
            }
            b'"' => {
                tokens.push(Token::Other);
                self.inside = Inside::Str;
                index + 1
            }
            b'\'' => {
                tokens.push(Token::Other);
                quote(bytes, index)
            }
            b'{' | b'(' | b'[' => {
                self.opener(byte, indent, tokens);
                index + 1
            }
            b'}' | b')' | b']' => {
                let (token, operand) = self.pop(byte, tokens.last == Some(Token::OpenBrace));
                tokens.push(token);
                tokens.popped = operand.or(tokens.popped);
                index + 1
            }
            b'?' => {
                tokens.push(Token::Question);
                index + 1
            }
            b'=' if !matches!(next, Some(b'=' | b'>'))
                && !index
                    .checked_sub(1)
                    .is_some_and(|before| b"=<>!+-*/%^&|".contains(&bytes[before])) =>
            {
                tokens.push(Token::Assign);
                index + 1
            }
            b';' => {
                self.end_clause();
                tokens.push(Token::Semicolon);
                index + 1
            }
            b',' => {
                tokens.push(Token::Comma);
                index + 1
            }
            b'#' => {
                tokens.push(Token::Other);
                tokens.hash = 1;
                index + 1
            }
            b'!' if tokens.hash == 1 => {
                tokens.hash = 2;
                index + 1
            }
            _ if is_word_byte(byte) => self.word(bytes, index, tokens),
            _ => {
                tokens.push(Token::Other);
                index + 1
            }
        };
        Some(after)
    }

    /// Opens the bracket `byte`: an attribute after `#` or `#!`, a `match`'s arms after
    /// the word.
    fn opener(&mut self, byte: u8, indent: usize, tokens: &mut Tokens) {
        let attribute = byte == b'[' && tokens.hash > 0;
        let arms = byte == b'{' && tokens.matched;
        tokens.matched &= byte != b'{';
        self.push(byte, indent, attribute, arms);
        tokens.push(if byte == b'{' {
            Token::OpenBrace
        } else {
            Token::Open
        });
    }

    /// Lexes a word at `index`, or the string or char literal it prefixes.
    fn word(&mut self, bytes: &[u8], index: usize, tokens: &mut Tokens) -> usize {
        let mut quote_at = index;
        if matches!(bytes[index], b'b' | b'c') {
            quote_at += 1;
        }
        match bytes.get(quote_at) {
            Some(b'"') if quote_at > index => {
                tokens.push(Token::Other);
                self.inside = Inside::Str;
                return quote_at + 1;
            }
            Some(b'\'') if quote_at > index && bytes[index] == b'b' => {
                tokens.push(Token::Other);
                return quote(bytes, quote_at);
            }
            Some(b'r') => {
                let hashes = bytes[quote_at + 1..]
                    .iter()
                    .take_while(|&&b| b == b'#')
                    .count();
                if bytes.get(quote_at + 1 + hashes) == Some(&b'"') {
                    tokens.push(Token::Other);
                    self.inside = Inside::Raw(hashes);
                    return quote_at + hashes + 2;
                }
            }
            _ => {}
        }
        let end = bytes[index..]
            .iter()
            .position(|&byte| !is_word_byte(byte))
            .map_or(bytes.len(), |length| index + length);
        if &bytes[index..end] == b"where" {
            self.set_clause();
            tokens.push(Token::Where);
        } else if &bytes[index..end] == b"match" {
            tokens.push(Token::Other);
            tokens.matched = true;
        } else {
            tokens.push(Token::Other);
        }
        end
    }

    /// Lexes inside a string from `index`, and returns where it goes on.
    fn string(&mut self, bytes: &[u8], mut index: usize) -> usize {
        while let Some(found) = bytes
            .get(index..)
            .and_then(|rest| rest.iter().position(|&byte| matches!(byte, b'"' | b'\\')))
        {
            index += found;
            if bytes[index] == b'"' {
                self.inside = Inside::Code;
                return index + 1;
            }
            index += 2;
        }
        bytes.len()
    }

    /// Lexes inside a raw string closed by `"` and `hashes` `#`.
    fn raw(&mut self, bytes: &[u8], mut index: usize, hashes: usize) -> usize {
        while let Some(quote) = bytes[index..].iter().position(|&byte| byte == b'"') {
            index += quote + 1;
            let closing = bytes[index..]
                .iter()
                .take(hashes)
                .take_while(|&&b| b == b'#')
                .count();
            if closing == hashes {
                self.inside = Inside::Code;
                return index + hashes;
            }
        }
        bytes.len()
    }

    /// Lexes inside block comments nested `depth` deep.
    fn comment(&mut self, bytes: &[u8], mut index: usize, mut depth: u32, column: usize) -> usize {
        while index + 1 < bytes.len() {
            // Only `/` and `*` can open or close one.
            match bytes[index..]
                .iter()
                .position(|&byte| matches!(byte, b'/' | b'*'))
            {
                Some(found) if index + found + 1 < bytes.len() => index += found,
                _ => break,
            }
            match (bytes[index], bytes[index + 1]) {
                (b'/', b'*') => {
                    depth = depth.saturating_add(1);
                    index += 2;
                }
                (b'*', b'/') => {
                    depth -= 1;
                    index += 2;
                    if depth == 0 {
                        self.inside = Inside::Code;
                        return index;
                    }
                }
                _ => index += 1,
            }
        }
        self.inside = Inside::Comment { depth, column };
        bytes.len()
    }

    fn push(&mut self, byte: u8, indent: usize, attribute: bool, arms: bool) {
        let kind = match byte {
            b'{' => Kind::Brace,
            b'(' => Kind::Paren,
            _ => Kind::Bracket,
        };
        if kind == Kind::Brace {
            // The body after a `where` clause ends it.
            self.end_clause();
        }
        if self.deeper > 0 || self.open.len() == MAX_DEPTH {
            self.deeper = self.deeper.saturating_add(1);
            return;
        }
        self.open.push(Open {
            kind,
            indent,
            line: self.lines,
            operand: self.operand,
            attribute,
            clause: false,
            arms,
        });
    }

    /// Closes the innermost bracket. `empty` says whether the token before was `{`.
    /// Also returns where an operator line goes after it, from its opener's line.
    fn pop(&mut self, byte: u8, empty: bool) -> (Token, Option<usize>) {
        if self.deeper > 0 {
            self.deeper -= 1;
            return (Token::Other, None);
        }
        let Some(open) = self.open.pop() else {
            let kind = match byte {
                b'}' => Kind::Brace,
                b')' => Kind::Paren,
                _ => Kind::Bracket,
            };
            return (Token::Close(kind), None);
        };
        let token = if open.attribute {
            Token::CloseAttribute
        } else if open.kind == Kind::Brace && open.line == self.lines && !empty {
            Token::CloseInline
        } else {
            Token::Close(open.kind)
        };
        (token, Some(open.operand))
    }
}

/// Lexes a char literal or a lifetime at the quote at `index`, and returns where it goes on.
fn quote(bytes: &[u8], index: usize) -> usize {
    match bytes.get(index + 1) {
        Some(b'\\') => {
            // The escaped character, then the closing quote.
            let from = (index + 3).min(bytes.len());
            let end = (from + MAX_CHAR).min(bytes.len());
            match bytes[from..end].iter().position(|&byte| byte == b'\'') {
                Some(close) => from + close + 1,
                None => index + 1,
            }
        }
        Some(&byte) => {
            let length = utf8_length(byte);
            if bytes.get(index + 1 + length) == Some(&b'\'') {
                index + 2 + length
            } else {
                // A lifetime or a label: its name is a word.
                let name = bytes[index + 1..]
                    .iter()
                    .take_while(|&&b| is_word_byte(b))
                    .count();
                index + 1 + name.max(1)
            }
        }
        None => index + 1,
    }
}

/// The length of the UTF-8 character that starts with `byte`.
fn utf8_length(byte: u8) -> usize {
    match byte {
        0xf0.. => 4,
        0xe0.. => 3,
        0xc0.. => 2,
        _ => 1,
    }
}
