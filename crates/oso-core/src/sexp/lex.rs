//! The strings and comments of one line, for a buffer with no highlights (ADR 0190): each line
//! is lexed from its own start with its mode's table, so a block comment or a string that goes
//! past its line is not seen.
//!
//! Must not depend on: `window` or `word`; it reads the bytes of one line.

use std::ops::Range;

use super::{Kind, Table};

/// The longest line lexed. A longer one is all code, so that a line has a bounded cost.
pub(super) const LINE_MAX: usize = 1 << 16;

/// What a table's lines hold.
struct Rules {
    /// The quotes of strings, each with whether a backslash escapes in it.
    quotes: &'static [(u8, bool)],
    /// What starts a comment to the line's end.
    line_comment: Option<&'static [u8]>,
    /// A block comment's opener and closer, on one line.
    block: Option<(&'static [u8], &'static [u8])>,
}

type Block = Option<(&'static [u8], &'static [u8])>;

const C_BLOCK: Block = Some((b"/*", b"*/"));
const HTML_BLOCK: Block = Some((b"<!--", b"-->"));
const SLASHES: Option<&[u8]> = Some(b"//");
const HASH: Option<&[u8]> = Some(b"#");
const PERCENT: Option<&[u8]> = Some(b"%");

fn rules(table: Table) -> Rules {
    let (quotes, line_comment, block): (&'static [(u8, bool)], _, _) = match table {
        Table::Fundamental => (&[], None, None),
        // Not `'`: Rust's lifetimes have no closer.
        Table::Rust | Table::Swift => (&[(b'"', true)], SLASHES, C_BLOCK),
        Table::Go => (
            &[(b'"', true), (b'\'', true), (b'`', false)],
            SLASHES,
            C_BLOCK,
        ),
        Table::Script => (
            &[(b'"', true), (b'\'', true), (b'`', true)],
            SLASHES,
            C_BLOCK,
        ),
        Table::Css => (&[(b'"', true), (b'\'', true)], None, C_BLOCK),
        Table::Json => (&[(b'"', true)], SLASHES, C_BLOCK),
        Table::Html => (&[], None, HTML_BLOCK),
        Table::Markdown => (&[(b'`', false)], None, None),
        Table::Toml => (&[(b'"', true), (b'\'', false)], HASH, None),
        Table::Yaml => (&[(b'"', true)], HASH, None),
        Table::Latex => (&[], PERCENT, None),
    };
    Rules {
        quotes,
        line_comment,
        block,
    }
}

/// Whether a table's lines are lexed at all: Fundamental's are brackets only.
pub(super) fn lexes(table: Table) -> bool {
    table != Table::Fundamental
}

/// Whether a line comment's marker at `at` starts one: YAML's `#` only at the line's start or
/// after a blank, and LaTeX's `%` unless an odd run of backslashes escapes it.
fn comment_starts(table: Table, line: &[u8], at: usize) -> bool {
    match table {
        Table::Yaml => at == 0 || matches!(line[at - 1], b' ' | b'\t'),
        Table::Latex => line[..at].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 0,
        _ => true,
    }
}

/// Puts the runs of `line` (bytes from a line's start, its line break last if it has one) in
/// `out`, offset by `base`: contiguous, none empty, from `base` to `base + line.len()`.
pub(super) fn lex(table: Table, line: &[u8], base: usize, out: &mut Vec<(Range<usize>, Kind)>) {
    let rules = rules(table);
    let text_end = line.len() - usize::from(line.last() == Some(&b'\n'));
    let mut push = |range: Range<usize>, kind: Kind| {
        if !range.is_empty() {
            out.push((base + range.start..base + range.end, kind));
        }
    };
    let (mut at, mut code) = (0, 0);
    while at < text_end {
        let rest = &line[at..text_end];
        if let Some((open, close)) = rules.block
            && rest.starts_with(open)
        {
            let end = find(&line[at + open.len()..text_end], close)
                .map_or(text_end, |found| at + open.len() + found + close.len());
            push(code..at, Kind::Code);
            push(at..end, Kind::Comment);
            (at, code) = (end, end);
            continue;
        }
        if let Some(marker) = rules.line_comment
            && rest.starts_with(marker)
            && comment_starts(table, line, at)
        {
            push(code..at, Kind::Code);
            push(at..text_end, Kind::Comment);
            code = text_end;
            break;
        }
        if let Some(&(quote, escapes)) = rules.quotes.iter().find(|(quote, _)| rest[0] == *quote)
            && let Some(end) = string_end(&line[..text_end], at, quote, escapes)
        {
            push(code..at, Kind::Code);
            push(at..end, Kind::Str);
            (at, code) = (end, end);
            continue;
        }
        at += 1;
    }
    push(code..line.len(), Kind::Code);
}

/// Where the string opened by the quote at `at` ends, past its closer, or `None` if the line
/// has no closer: then the quote is no string's.
fn string_end(line: &[u8], at: usize, quote: u8, escapes: bool) -> Option<usize> {
    let mut i = at + 1;
    while i < line.len() {
        match line[i] {
            b'\\' if escapes => i += 2,
            b if b == quote => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(table: Table, line: &str) -> Vec<(&str, Kind)> {
        let mut out = Vec::new();
        lex(table, line.as_bytes(), 0, &mut out);
        out.into_iter()
            .map(|(range, kind)| (&line[range], kind))
            .collect()
    }

    #[test]
    fn strings_and_comments_of_a_line() {
        use Kind::{Code, Comment, Str};
        assert_eq!(
            runs(Table::Swift, "f(\"a)\\\"b\") // c(\n"),
            [
                ("f(", Code),
                ("\"a)\\\"b\"", Str),
                (") ", Code),
                ("// c(", Comment),
                ("\n", Code)
            ]
        );
        assert_eq!(
            runs(Table::Script, "a /* ( */ b /* (\n"),
            [
                ("a ", Code),
                ("/* ( */", Comment),
                (" b ", Code),
                ("/* (", Comment),
                ("\n", Code)
            ]
        );
        assert_eq!(runs(Table::Rust, "x = \"(;"), [("x = \"(;", Code)]);
        assert_eq!(runs(Table::Rust, "&'a x"), [("&'a x", Code)]);
        assert_eq!(
            runs(Table::Fundamental, "\"(\" // x"),
            [("\"(\" // x", Code)]
        );
    }

    #[test]
    fn a_comment_marker_counts_where_its_mode_says() {
        use Kind::{Code, Comment};
        assert_eq!(
            runs(Table::Latex, r"50\% (a) % b"),
            [(r"50\% (a) ", Code), ("% b", Comment)]
        );
        assert_eq!(
            runs(Table::Latex, r"\\% b"),
            [(r"\\", Code), ("% b", Comment)]
        );
        assert_eq!(
            runs(Table::Yaml, "a#b # c"),
            [("a#b ", Code), ("# c", Comment)]
        );
        assert_eq!(
            runs(Table::Toml, "a = 'b\\' # c"),
            [
                ("a = ", Code),
                ("'b\\'", Kind::Str),
                (" ", Code),
                ("# c", Comment)
            ]
        );
    }
}
