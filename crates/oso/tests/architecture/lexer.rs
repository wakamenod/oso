//! A small Rust lexer for the architecture checks. It drops comments, string
//! and character literals, lifetimes and numbers, so the checks see only
//! identifiers and punctuation, each with its line.

/// A token the checks look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    /// `::`
    PathSep,
    Punct(char),
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    /// 1-based.
    pub line: usize,
}

impl Token {
    pub fn is_ident(&self, name: &str) -> bool {
        matches!(&self.tok, Tok::Ident(ident) if ident == name)
    }

    pub fn is_punct(&self, c: char) -> bool {
        self.tok == Tok::Punct(c)
    }
}

/// The tokens of `src`.
pub fn tokenize(src: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        chars: src.chars().collect(),
        pos: 0,
        line: 1,
        tokens: Vec::new(),
    };
    lexer.run();
    lexer.tokens
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    tokens: Vec<Token>,
}

impl Lexer {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek(0)?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
        }
        Some(c)
    }

    fn push(&mut self, tok: Tok, line: usize) {
        self.tokens.push(Token { tok, line });
    }

    fn run(&mut self) {
        while let Some(c) = self.peek(0) {
            let line = self.line;
            match c {
                '/' if self.peek(1) == Some('/') => self.skip_line(),
                '/' if self.peek(1) == Some('*') => self.skip_block_comment(),
                '"' => self.skip_string(),
                '\'' => self.skip_char_or_lifetime(),
                ':' if self.peek(1) == Some(':') => {
                    self.pos += 2;
                    self.push(Tok::PathSep, line);
                }
                c if c.is_ascii_digit() => self.skip_word(),
                c if is_ident_char(c) => self.word(line),
                c if c.is_whitespace() => {
                    self.bump();
                }
                c => {
                    self.bump();
                    self.push(Tok::Punct(c), line);
                }
            }
        }
    }

    fn skip_line(&mut self) {
        while self.peek(0).is_some_and(|c| c != '\n') {
            self.bump();
        }
    }

    /// Block comments nest in Rust.
    fn skip_block_comment(&mut self) {
        let mut depth = 0usize;
        while let Some(c) = self.bump() {
            if c == '/' && self.peek(0) == Some('*') {
                self.bump();
                depth += 1;
            } else if c == '*' && self.peek(0) == Some('/') {
                self.bump();
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
        }
    }

    /// A string, or a byte or C string after its prefix.
    fn skip_string(&mut self) {
        self.bump();
        while let Some(c) = self.bump() {
            match c {
                '\\' => {
                    self.bump();
                }
                '"' => return,
                _ => {}
            }
        }
    }

    /// A raw string from its `#`s; the prefix is already consumed.
    fn skip_raw_string(&mut self) {
        let mut hashes = 0;
        while self.peek(0) == Some('#') {
            self.bump();
            hashes += 1;
        }
        self.bump();
        while let Some(c) = self.bump() {
            if c == '"' && (0..hashes).all(|i| self.peek(i) == Some('#')) {
                self.pos += hashes;
                return;
            }
        }
    }

    /// `'a'`, `'\n'` and `'\u{..}'` are characters; `'a` is a lifetime.
    fn skip_char_or_lifetime(&mut self) {
        self.bump();
        if self.peek(0) == Some('\\') {
            self.bump();
            self.bump();
            while self.bump().is_some_and(|c| c != '\'') {}
        } else if self.peek(1) == Some('\'') {
            self.pos += 2;
        } else {
            self.skip_word();
        }
    }

    fn skip_word(&mut self) {
        while self.peek(0).is_some_and(is_ident_char) {
            self.bump();
        }
    }

    /// An identifier, a raw identifier, or a prefixed literal (`b"…"`,
    /// `r#"…"#`, `b'…'`).
    fn word(&mut self, line: usize) {
        let start = self.pos;
        self.skip_word();
        let word: String = self.chars[start..self.pos].iter().collect();
        let next = self.peek(0);
        let raw_prefix = matches!(word.as_str(), "r" | "br" | "cr");
        if raw_prefix && (next == Some('"') || self.raw_string_hashes()) {
            self.skip_raw_string();
        } else if matches!(word.as_str(), "b" | "c") && next == Some('"') {
            self.skip_string();
        } else if word == "b" && next == Some('\'') {
            self.skip_char_or_lifetime();
        } else if word == "r" && next == Some('#') {
            self.bump();
            let start = self.pos;
            self.skip_word();
            let ident = self.chars[start..self.pos].iter().collect();
            self.push(Tok::Ident(ident), line);
        } else {
            self.push(Tok::Ident(word), line);
        }
    }

    /// Whether `#`s followed by `"` start here.
    fn raw_string_hashes(&self) -> bool {
        let mut i = 0;
        while self.peek(i) == Some('#') {
            i += 1;
        }
        i > 0 && self.peek(i) == Some('"')
    }
}

/// The index just past the group that opens at `open` (`(`, `[` or `{`), or
/// the end.
pub fn skip_group(tokens: &[Token], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate().skip(open) {
        match token.tok {
            Tok::Punct('(' | '[' | '{') => depth += 1,
            Tok::Punct(')' | ']' | '}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
    }
    tokens.len()
}

/// Whether the `cfg` predicate in `tokens` (between the parentheses) holds
/// only under test: `test`, or `all(…)` with `test` as an argument.
fn is_test_predicate(tokens: &[Token]) -> bool {
    match tokens {
        [only] => only.is_ident("test"),
        [all, open, args @ ..] if all.is_ident("all") && open.is_punct('(') => {
            let mut depth = 0usize;
            args.iter().any(|token| {
                match token.tok {
                    Tok::Punct('(') => depth += 1,
                    Tok::Punct(')') => depth = depth.saturating_sub(1),
                    _ => {}
                }
                depth == 0 && token.is_ident("test")
            })
        }
        _ => false,
    }
}

/// If a `#[cfg(<test-only>)]` attribute starts at `i`, the index past it.
fn test_attribute_end(tokens: &[Token], i: usize) -> Option<usize> {
    let is = |k: usize, f: &dyn Fn(&Token) -> bool| tokens.get(i + k).is_some_and(f);
    let attribute = is(0, &|t| t.is_punct('#'))
        && is(1, &|t| t.is_punct('['))
        && is(2, &|t| t.is_ident("cfg"))
        && is(3, &|t| t.is_punct('('));
    if !attribute {
        return None;
    }
    let close = skip_group(tokens, i + 3);
    let predicate = tokens.get(i + 4..close - 1)?;
    let end = skip_group(tokens, i + 1);
    is_test_predicate(predicate).then_some(end)
}

/// The index of the last token of the item that starts at `start`: its `;`
/// or the `}` closing its body. Attributes and bracketed types are skipped.
fn item_end(tokens: &[Token], start: usize) -> usize {
    let mut i = start;
    while let Some(token) = tokens.get(i) {
        match token.tok {
            Tok::Punct('#') if tokens.get(i + 1).is_some_and(|t| t.is_punct('[')) => {
                i = skip_group(tokens, i + 1);
            }
            Tok::Punct('(' | '[') => i = skip_group(tokens, i),
            Tok::Punct('{') => return skip_group(tokens, i) - 1,
            Tok::Punct(';') => return i,
            _ => i += 1,
        }
    }
    tokens.len().saturating_sub(1)
}

/// The line ranges (inclusive) of top-level items under a test-only `cfg`,
/// such as `#[cfg(test)] mod tests { … }`.
pub fn test_item_lines(tokens: &[Token]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(token) = tokens.get(i) {
        if depth == 0
            && let Some(after) = test_attribute_end(tokens, i)
        {
            let end = item_end(tokens, after);
            ranges.push((token.line, tokens[end].line));
            i = end + 1;
            continue;
        }
        match token.tok {
            Tok::Punct('{') => depth += 1,
            Tok::Punct('}') => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::{Tok, test_item_lines, tokenize};

    fn idents(src: &str) -> Vec<String> {
        tokenize(src)
            .into_iter()
            .filter_map(|t| match t.tok {
                Tok::Ident(name) => Some(name),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn comments_and_literals_are_dropped() {
        let src = r##"
            // crate::hidden_a
            /* crate::hidden_b /* nested */ crate::hidden_c */
            let s = "crate::hidden_d \" crate::hidden_e";
            let r = r#"crate::hidden_f "# ;
            let b = b"crate::hidden_g";
            let c = '"'; let q = '\''; let u = '\u{1F600}';
            fn func<'hidden_h>(x: &'hidden_h str) {}
            keep
        "##;
        let names = idents(src);
        assert!(!names.iter().any(|n| n.starts_with("hidden")), "{names:?}");
        assert!(names.contains(&"keep".to_owned()));
    }

    #[test]
    fn raw_identifiers_and_lines() {
        let tokens = tokenize("r#type\n::x");
        assert_eq!(tokens[0].tok, Tok::Ident("type".to_owned()));
        assert_eq!(tokens[1].tok, Tok::PathSep);
        assert_eq!(tokens[1].line, 2);
    }

    #[test]
    fn test_items_are_found_at_the_top_level_only() {
        let src = "fn a() {}\n\
                   #[cfg(test)]\n\
                   mod tests {\n\
                   fn t() {}\n\
                   }\n\
                   #[cfg(all(test, unix))]\n\
                   use x;\n\
                   #[cfg(any(test, feature = \"dev\"))]\n\
                   fn shared() {}\n\
                   mod m {\n\
                   #[cfg(test)]\n\
                   fn inner() {}\n\
                   }\n";
        assert_eq!(test_item_lines(&tokenize(src)), [(2, 5), (6, 7)]);
    }
}
