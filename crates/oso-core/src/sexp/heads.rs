//! The lines a defun starts on, in each mode's way (ADR 0190, Q2): a line's first bytes tell
//! whether it is a defun's head, how deep it sits and how its defun ends.
//!
//! Must not depend on: `window` or `word`; it reads the bytes of one line.

use super::Table;

/// The first bytes of a line read to match a head. A head indented further is not seen.
pub(super) const HEAD_MAX: usize = 512;

/// How a defun ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Ends {
    /// After the bracket group from the first `{` past its head, or at a `;` before any,
    /// both outside brackets; or before a blank line or the next head, if either comes first.
    Braces,
    /// As `Braces`, but `[` opens its group too and `,` ends it: a JSON key's value.
    Value,
    /// After the bracket group its head opens.
    Group,
    /// Before the next head of its level or a higher one.
    Level,
    /// After the line of the `\end{…}` that closes this `\begin{…}`.
    Env(Box<[u8]>),
}

/// A defun's head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Head {
    /// The bytes of blanks before it.
    pub(super) indent: usize,
    /// How deep it sits: a head holds those of a greater level that follow it.
    pub(super) level: usize,
    pub(super) ends: Ends,
}

/// What a line holds, as a head is matched on it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Line<'a> {
    /// Its first bytes, at most [`HEAD_MAX`], with no line break.
    pub(super) start: &'a [u8],
    /// Its last byte that is not a blank, if it has one.
    pub(super) last: Option<u8>,
}

/// The head `line` starts, if it is one. `json_indent` is the indentation of a JSON
/// buffer's top-level keys.
pub(super) fn head(table: Table, line: Line<'_>, json_indent: Option<usize>) -> Option<Head> {
    let indent = line
        .start
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let text = &line.start[indent..];
    let braces = |found: bool| {
        found.then_some(Head {
            indent,
            level: indent,
            ends: Ends::Braces,
        })
    };
    let level = |level: usize| Head {
        indent,
        level,
        ends: Ends::Level,
    };
    match table {
        Table::Rust => braces(rust(text, line.last)),
        // gofmt puts every declaration at the line's start; `var` in a body is no head.
        Table::Go => braces(indent == 0 && keyword(text, &[b"func", b"type", b"var", b"const"])),
        Table::Script => braces(script(text)),
        Table::Swift => braces(swift(text)),
        Table::Css => braces(!text.is_empty() && line.last == Some(b'{')),
        Table::Toml => toml(text).then(|| level(0)),
        Table::Yaml => (indent == 0 && yaml(text)).then(|| level(0)),
        Table::Json => (Some(indent) == json_indent && json_key(text).is_some()).then_some(Head {
            indent,
            level: indent,
            ends: Ends::Value,
        }),
        Table::Markdown => markdown(text, indent).map(level),
        Table::Latex => latex(text, indent),
        Table::Fundamental => {
            (indent == 0 && matches!(text.first(), Some(b'(' | b'{'))).then_some(Head {
                indent,
                level: 0,
                ends: Ends::Group,
            })
        }
        Table::Html => None,
    }
}

/// Whether `b` may be in an identifier.
fn word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || !b.is_ascii()
}

/// `text` past `word` and the blanks after it, if it starts with that word.
fn after_word<'a>(text: &'a [u8], word: &[u8]) -> Option<&'a [u8]> {
    let rest = text.strip_prefix(word)?;
    if rest.first().is_some_and(|&b| word_byte(b)) {
        return None;
    }
    Some(blanks(rest))
}

fn blanks(text: &[u8]) -> &[u8] {
    let skip = text
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    &text[skip..]
}

/// Whether `text` starts with one of `words`.
fn keyword(text: &[u8], words: &[&[u8]]) -> bool {
    words.iter().any(|word| after_word(text, word).is_some())
}

/// `text` past a bracket group it starts with, `(…)` with no `(` inside, and the blanks
/// after it.
fn after_parens(text: &[u8]) -> Option<&[u8]> {
    let rest = text.strip_prefix(b"(")?;
    let close = rest.iter().position(|&b| b == b')')?;
    Some(blanks(&rest[close + 1..]))
}

fn rust(mut text: &[u8], last: Option<u8>) -> bool {
    loop {
        if let Some(rest) = after_word(text, b"pub") {
            text = after_parens(rest).unwrap_or(rest);
        } else if let Some(rest) =
            after_word(text, b"async").or_else(|| after_word(text, b"unsafe"))
        {
            text = rest;
        } else if let Some(rest) = after_word(text, b"extern") {
            // `extern "C"`, its ABI quoted; `extern crate` is no item.
            let Some(quoted) = rest.strip_prefix(b"\"") else {
                return false;
            };
            let Some(close) = quoted.iter().position(|&b| b == b'"') else {
                return false;
            };
            text = blanks(&quoted[close + 1..]);
        } else if let Some(rest) = after_word(text, b"const")
            && keyword(rest, &[b"fn", b"unsafe", b"async", b"extern"])
        {
            text = rest;
        } else {
            break;
        }
    }
    if text.starts_with(b"macro_rules!") {
        return true;
    }
    if let Some(rest) = after_word(text, b"mod") {
        // `mod m;` names a file: only a `mod` with its items here is a head.
        let name = rest.iter().take_while(|&&b| word_byte(b)).count();
        let after = blanks(&rest[name..]);
        return !(after.starts_with(b";") || (after.is_empty() && last == Some(b';')));
    }
    // `union` is a keyword only before a name.
    if let Some(rest) = after_word(text, b"union") {
        return rest.first().is_some_and(|&b| word_byte(b));
    }
    keyword(
        text,
        &[
            b"fn", b"struct", b"enum", b"trait", b"impl", b"const", b"static", b"type",
        ],
    )
}

fn script(text: &[u8]) -> bool {
    if keyword(text, &[b"function", b"class", b"interface", b"export"]) {
        return true;
    }
    if let Some(rest) = after_word(text, b"async") {
        return keyword(rest, &[b"function"]);
    }
    // `const f = (…) =>` and `const f = async (…) =>`.
    let Some(rest) = after_word(text, b"const") else {
        return false;
    };
    let name = rest
        .iter()
        .take_while(|&&b| word_byte(b) || b == b'$')
        .count();
    let rest = blanks(&rest[name..]);
    let Some(rest) = rest.strip_prefix(b"=") else {
        return false;
    };
    let rest = blanks(rest);
    let rest = after_word(rest, b"async").unwrap_or(rest);
    name > 0 && after_parens(rest).is_some_and(|rest| rest.starts_with(b"=>"))
}

fn swift(mut text: &[u8]) -> bool {
    const MODIFIERS: &[&[u8]] = &[
        b"public",
        b"private",
        b"fileprivate",
        b"internal",
        b"open",
        b"final",
        b"static",
        b"class",
        b"override",
        b"mutating",
        b"nonmutating",
        b"indirect",
        b"convenience",
        b"required",
        b"dynamic",
        b"lazy",
    ];
    const HEADS: &[&[u8]] = &[
        b"func",
        b"class",
        b"struct",
        b"enum",
        b"extension",
        b"protocol",
    ];
    loop {
        if let Some(rest) = text.strip_prefix(b"@") {
            // An attribute, `@objc` or `@available(…)`.
            let name = rest.iter().take_while(|&&b| word_byte(b)).count();
            let rest = &rest[name..];
            text = blanks(after_parens(rest).unwrap_or(rest));
            if name == 0 {
                return false;
            }
        } else if keyword(text, HEADS) {
            return true;
        } else if let Some(rest) = MODIFIERS.iter().find_map(|word| after_word(text, word)) {
            text = rest;
        } else {
            return false;
        }
    }
}

/// `[table]` or `[[table]]`, with nothing but a comment after.
fn toml(text: &[u8]) -> bool {
    let Some(rest) = text.strip_prefix(b"[") else {
        return false;
    };
    let rest = rest.strip_prefix(b"[").unwrap_or(rest);
    let Some(close) = rest.iter().position(|&b| b == b']') else {
        return false;
    };
    let key = &rest[..close];
    let after = rest[close..].iter().skip_while(|&&b| b == b']');
    let after: Vec<u8> = after.copied().collect();
    let after = blanks(&after);
    !key.is_empty()
        && key
            .iter()
            .all(|&b| word_byte(b) || matches!(b, b'-' | b'.' | b'"' | b'\'' | b' '))
        && key.iter().any(|&b| !b.is_ascii_digit() && b != b' ')
        && (after.is_empty() || after[0] == b'#')
}

/// A key at the line's start: a plain or quoted key and a `:` before a blank or the end.
fn yaml(text: &[u8]) -> bool {
    if matches!(
        text.first(),
        None | Some(b'#' | b'-' | b'%' | b'[' | b'{' | b']' | b'}' | b'.' | b'|' | b'>')
    ) {
        return false;
    }
    let key_end = match text[0] {
        quote @ (b'"' | b'\'') => match text[1..].iter().position(|&b| b == quote) {
            Some(close) => close + 2,
            None => return false,
        },
        _ => 0,
    };
    text[key_end..]
        .windows(2)
        .position(|pair| pair[0] == b':' && (pair[1] == b' ' || pair[1] == b'\t'))
        .is_some()
        || text.ends_with(b":")
}

/// The end of a quoted key and its `:`, as `"a": 1` starts.
fn json_key(text: &[u8]) -> Option<usize> {
    let rest = text.strip_prefix(b"\"")?;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            b'\\' => i += 2,
            b'"' => break,
            _ => i += 1,
        }
    }
    let after = rest.get(i + 1..)?;
    let colon = blanks(after).strip_prefix(b":")?;
    Some(text.len() - colon.len())
}

/// An ATX heading's level, 1 to 6: at most 3 blanks before it, and a blank or the end
/// after its marks.
fn markdown(text: &[u8], indent: usize) -> Option<usize> {
    let marks = text.iter().take_while(|&&b| b == b'#').count();
    let after = text.get(marks);
    (indent <= 3 && (1..=6).contains(&marks) && after.is_none_or(|&b| b == b' ' || b == b'\t'))
        .then_some(marks)
}

fn latex(text: &[u8], indent: usize) -> Option<Head> {
    const SECTIONS: &[&[u8]] = &[
        b"part",
        b"chapter",
        b"section",
        b"subsection",
        b"subsubsection",
        b"paragraph",
        b"subparagraph",
    ];
    let command = text.strip_prefix(b"\\")?;
    if let Some(rest) = command.strip_prefix(b"begin{") {
        let close = rest.iter().position(|&b| b == b'}')?;
        return (close > 0).then(|| Head {
            indent,
            level: SECTIONS.len(),
            ends: Ends::Env(rest[..close].into()),
        });
    }
    let name = command
        .iter()
        .take_while(|b| b.is_ascii_alphabetic())
        .count();
    let level = SECTIONS
        .iter()
        .position(|section| *section == &command[..name])?;
    let rest = &command[name..];
    let rest = rest.strip_prefix(b"*").unwrap_or(rest);
    matches!(rest.first(), Some(b'{' | b'[')).then_some(Head {
        indent,
        level,
        ends: Ends::Level,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Line<'_> {
        Line {
            start: text.as_bytes(),
            last: text.bytes().rev().find(|b| !b.is_ascii_whitespace()),
        }
    }

    fn is_head(table: Table, text: &str) -> bool {
        head(table, line(text), Some(2)).is_some()
    }

    #[track_caller]
    fn heads(table: Table, yes: &[&str], no: &[&str]) {
        for text in yes {
            assert!(is_head(table, text), "{table:?} head: {text:?}");
        }
        for text in no {
            assert!(!is_head(table, text), "{table:?} not a head: {text:?}");
        }
    }

    #[test]
    fn rust_items_after_their_qualifiers() {
        heads(
            Table::Rust,
            &[
                "fn a() {",
                "    pub fn a(&self) -> u32 {",
                "pub(crate) async unsafe fn a() {",
                "pub(in crate::x) struct S;",
                "extern \"C\" fn a() {}",
                "const fn a() {",
                "pub const unsafe fn a() {",
                "enum E {",
                "union U {",
                "pub trait T<A> {",
                "impl<T> S<T> {",
                "unsafe impl Send for S {}",
                "mod m {",
                "const C: u32 = 1;",
                "static S: u32 = 1;",
                "pub type T = u32;",
                "macro_rules! m {",
                "\tfn tabbed() {",
            ],
            &[
                "use a::b;",
                "pub use a::b;",
                "mod m;",
                "pub mod m; // A comment.",
                "let fn_x = 1;",
                "fnord()",
                "// fn a() {",
                "extern crate a;",
                "union(a, b)",
                "impls {",
                "",
            ],
        );
    }

    #[test]
    fn go_declarations_at_the_line_start() {
        heads(
            Table::Go,
            &[
                "func main() {",
                "func (s *S) M() {",
                "type S struct {",
                "var x = 1",
                "const (",
            ],
            &["\tvar x = 1", "\tfunc() {", "funcs()", "// func a()"],
        );
    }

    #[test]
    fn script_functions_classes_and_exports() {
        heads(
            Table::Script,
            &[
                "function a() {",
                "  function inner() {",
                "async function a() {",
                "class A extends B {",
                "export default class {",
                "export const a = 1;",
                "interface I {",
                "const f = (a, b) => {",
                "const f = async (a) => a;",
                "  const g = () => 1",
            ],
            &[
                "const a = 1;",
                "const f = (a, b);",
                "let f = () => 1",
                "functional()",
                "async () => {",
                "return function () {",
            ],
        );
    }

    #[test]
    fn swift_declarations_after_their_modifiers() {
        heads(
            Table::Swift,
            &[
                "func a() {",
                "    public func a() -> Int {",
                "final class A {",
                "@objc private static func a() {",
                "struct S {",
                "enum E {",
                "extension A: P {",
                "protocol P {",
                "fileprivate override func a() {",
            ],
            &["let a = 1", "funcs()", "return a"],
        );
    }

    #[test]
    fn css_rules_and_data_heads() {
        heads(
            Table::Css,
            &["a {", "  .b > c, d {", "@media (x) {"],
            &["  color: red;", "}", "a,"],
        );
        heads(
            Table::Toml,
            &["[table]", "[[array.of]]", "  [a.\"b c\"]  # Comment."],
            &["a = 1", "  [1, 2],", "[a, b]", "# [x]"],
        );
        heads(
            Table::Yaml,
            &["a:", "a: 1", "\"a b\": x", "a-b: [1]"],
            &["  a: 1", "- a: 1", "# a:", "---", "a", "http://x"],
        );
        // The top level's keys are indented by 2 here.
        heads(
            Table::Json,
            &["  \"a\": 1,", "  \"b\": {"],
            &["    \"c\": 2", "\"d\": 1", "  \"e\"", "  1,"],
        );
    }

    #[test]
    fn markdown_headings_and_latex_sections() {
        heads(
            Table::Markdown,
            &["# A", "## B", "   ###### C", "#"],
            &["#hashtag", "    # Code", "####### Seven", "text # no"],
        );
        let level = |text| head(Table::Markdown, line(text), None).map(|head| head.level);
        assert_eq!((level("# A"), level("### C")), (Some(1), Some(3)));
        heads(
            Table::Latex,
            &[
                "\\section{A}",
                "\\subsection*{B}",
                "  \\chapter[Short]{Long}",
                "\\begin{itemize}",
                "\\paragraph{P}",
            ],
            &[
                "\\sectionmark{A}",
                "% \\section{A}",
                "text \\section{A}",
                "\\end{x}",
            ],
        );
        let latex = |text| head(Table::Latex, line(text), None);
        assert!(latex("\\section{A}").unwrap().level < latex("\\subsection{A}").unwrap().level);
        assert_eq!(
            latex("\\begin{itemize} x").unwrap().ends,
            Ends::Env(b"itemize".to_vec().into())
        );
    }

    #[test]
    fn fundamental_at_column_0_and_what_each_ends_by() {
        heads(
            Table::Fundamental,
            &["(defun a ()", "{"],
            &[" (a", "a", "["],
        );
        assert_eq!(
            head(Table::Fundamental, line("(a"), None).unwrap().ends,
            Ends::Group
        );
        assert_eq!(
            head(Table::Rust, line("    fn a() {"), None),
            Some(Head {
                indent: 4,
                level: 4,
                ends: Ends::Braces
            })
        );
        assert_eq!(
            head(Table::Json, line("  \"a\": 1"), Some(2)).unwrap().ends,
            Ends::Value
        );
        assert_eq!(
            head(Table::Toml, line("[a]"), None).unwrap().ends,
            Ends::Level
        );
        // A JSON buffer with no keys found has no heads.
        assert_eq!(head(Table::Json, line("  \"a\": 1"), None), None);
        assert_eq!(head(Table::Html, line("<div>"), None), None);
    }
}
