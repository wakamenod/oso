//! Tests of motion by defuns: each mode's heads and ends, nested items, the comment above,
//! highlights, counts, and crafted costs.
//!
//! Must not depend on: `window` or `word`.

use proptest::prelude::*;

use super::*;
use crate::display_map::{DisplayMap, DisplaySnapshot, TabWidth};
use crate::rope::Rope;
use crate::sexp::Table;
use crate::syntax::{Highlight, Highlights};

fn display(text: &str) -> DisplaySnapshot {
    DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot()
}

/// Highlights giving each `(start, text, highlight)` its highlight at `start`.
fn highlights(text: &str, parts: &[(usize, &str, Highlight)]) -> Highlights {
    let mut highlights = Highlights::new(text.len());
    for &(at, part, highlight) in parts {
        assert_eq!(&text[at..at + part.len()], part);
        highlights.replace(at..at + part.len(), [(part.len(), Some(highlight))]);
    }
    highlights
}

type Motion = fn(&Syntax<'_>, &mut Selection, isize, &mut WalkBudget) -> Result<(), Edge>;

/// Runs `motion` from the `|` in `case` and shows where it went, and the edge it hit.
fn go_in(
    table: Table,
    case: &str,
    highlights: Option<&Highlights>,
    motion: Motion,
    n: isize,
) -> String {
    let from = case.find('|').expect("a | marks point");
    let text = case.replacen('|', "", 1);
    let display = display(&text);
    let syntax = Syntax::new(&display, highlights, table);
    let mut selection = Selection::at(from);
    let mut budget = WalkBudget::new();
    let result = motion(&syntax, &mut selection, n, &mut budget);
    assert!(!budget.is_spent());
    let shown = format!(
        "{}|{}",
        &text[..selection.cursor],
        &text[selection.cursor..]
    );
    match result {
        Ok(()) => shown,
        Err(edge) => format!("{shown} !{edge:?}"),
    }
}

fn go(table: Table, case: &str, motion: Motion, n: isize) -> String {
    go_in(table, case, None, motion, n)
}

/// Shows what `defun_bounds` gives at the `|` in `case`, in `[]`.
fn bounds_in(table: Table, case: &str, highlights: Option<&Highlights>) -> String {
    let from = case.find('|').expect("a | marks point");
    let text = case.replacen('|', "", 1);
    let display = display(&text);
    let syntax = Syntax::new(&display, highlights, table);
    let mut budget = WalkBudget::new();
    match defun_bounds(&syntax, from, &mut budget) {
        Some(range) => format!(
            "{}[{}]{}",
            &text[..range.start],
            &text[range.clone()],
            &text[range.end..]
        ),
        None => "None".to_owned(),
    }
}

fn bounds(table: Table, case: &str) -> String {
    bounds_in(table, case, None)
}

const BEGIN: Motion = beginning_of_defun;
const END: Motion = end_of_defun;

const RUST: &str = "use a;\n\
\n\
/// S.\n\
#[derive(Debug)]\n\
pub struct S {\n\
\x20   a: u32,\n\
}\n\
\n\
impl S {\n\
\x20   pub fn one(&self) {\n\
\x20       let s = \"}\";\n\
\x20       // }\n\
\x20   }\n\
\n\
\x20   // Two.\n\
\x20   fn two() {}\n\
}\n\
\n\
mod m;\n\
mod n {\n\
\x20   const C: [u8; 2] = [1, 2];\n\
}\n";

/// `RUST` with `|` at the first `find`, plus `after` bytes.
fn rust_at(find: &str, after: usize) -> String {
    let at = RUST.find(find).expect("in RUST") + after;
    format!("{}|{}", &RUST[..at], &RUST[at..])
}

/// The text of `RUST` up to where the `|` of `shown` is.
fn before_point(shown: &str) -> &str {
    &shown[..shown.find('|').expect("a | marks point")]
}

#[test]
fn beginning_of_defun_goes_to_the_head_before_at_any_indentation() {
    let begin = |find, after, n| go(Table::Rust, &rust_at(find, after), BEGIN, n);
    assert!(before_point(&begin("let s", 0, 1)).ends_with("impl S {\n    "));
    assert!(before_point(&begin("let s", 0, 2)).ends_with("\n\n"));
    assert!(
        begin("let s", 0, 2).contains("|impl S"),
        "{}",
        begin("let s", 0, 2)
    );
    // From a head's start, the one before; from inside its line, that head.
    assert!(begin("impl S", 0, 1).contains("|pub struct S"));
    assert!(begin("impl S", 2, 1).contains("|impl S"));
    // `mod m;` is no head, and `mod n {` is.
    assert!(begin("const C", 0, 1).contains("|mod n {"));
    assert!(begin("const C", 2, 1).contains("|const C"));
    // Nothing above: the buffer's start, and a step from there says so, as paragraphs do.
    assert_eq!(begin("pub struct", 0, 1), format!("|{RUST}"));
    assert_eq!(begin("pub struct", 0, 2), format!("|{RUST} !Start"));
    assert_eq!(
        go(Table::Rust, &format!("|{RUST}"), BEGIN, 1),
        format!("|{RUST} !Start")
    );
    assert_eq!(begin("pub struct", 0, 0), rust_at("pub struct", 0));
    // A count, and a negative one going forward to the next heads' starts.
    assert!(begin("two()", 0, 3).contains("|impl S"));
    assert!(begin("two()", 0, 4).contains("|pub struct S"));
    assert!(begin("use a", 0, -1).contains("|pub struct S"));
    assert!(begin("use a", 0, -3).contains("|pub fn one"));
    assert!(begin("mod n", 0, -1).ends_with("|const C: [u8; 2] = [1, 2];\n}\n"));
    assert_eq!(begin("mod n", 0, -2), format!("{RUST}|"));
    assert_eq!(begin("mod n", 0, -3), format!("{RUST}| !End"));
}

#[test]
fn end_of_defun_goes_past_the_end_of_the_one_at_point_or_the_next() {
    let end = |find, after, n| go(Table::Rust, &rust_at(find, after), END, n);
    let after_s = "    a: u32,\n}\n";
    assert!(before_point(&end("use a", 0, 1)).ends_with(after_s));
    assert!(before_point(&end("a: u32", 0, 1)).ends_with(after_s));
    // The brace in a string and in a comment do not close it.
    assert!(before_point(&end("let s", 0, 1)).ends_with("// }\n    }\n"));
    // Between methods, the next one; after the last, what holds it.
    assert!(before_point(&end("    // Two", 0, 1)).ends_with("fn two() {}\n"));
    assert!(before_point(&end("fn two", 0, 1)).ends_with("fn two() {}\n"));
    assert!(before_point(&end("fn two", 0, 2)).ends_with("fn two() {}\n}\n"));
    assert!(before_point(&end("mod m", 0, 1)).ends_with("[1, 2];\n}\n"));
    assert_eq!(end("mod n", 0, 1), format!("{RUST}|"));
    assert_eq!(end("mod n", 0, 2), format!("{RUST}| !End"));
    // Backward, to the ends of the ones before.
    assert!(before_point(&end("let s", 0, -1)).ends_with(after_s));
    assert!(before_point(&end("impl S", 0, -1)).ends_with(after_s));
    assert_eq!(end("pub struct", 0, -1), format!("|{RUST}"));
    assert_eq!(end("pub struct", 0, -2), format!("|{RUST} !Start"));
}

#[test]
fn a_defun_ends_at_a_semicolon_a_blank_line_or_the_next_head() {
    let end = |case| go(Table::Rust, case, END, 1);
    assert_eq!(end("|struct S;\nfn a() {}\n"), "struct S;\n|fn a() {}\n");
    assert_eq!(
        end("|fn a(\n    b: u32,\n) -> u32 where\n    u32: X,\n{\n    b\n}\nx"),
        "fn a(\n    b: u32,\n) -> u32 where\n    u32: X,\n{\n    b\n}\n|x"
    );
    assert_eq!(
        end("|const C: [u8; 2] = [1, 2];\nx"),
        "const C: [u8; 2] = [1, 2];\n|x"
    );
    let go_end = |case| go(Table::Go, case, END, 1);
    assert_eq!(
        go_end("|var x = 1\n\nfunc f() {\n}\n"),
        "var x = 1\n|\nfunc f() {\n}\n"
    );
    assert_eq!(
        go_end("|var (\n\ta = 1\n)\nfunc f() {\n}\n"),
        "var (\n\ta = 1\n)\n|func f() {\n}\n"
    );
    let script = |case| go(Table::Script, case, END, 1);
    assert_eq!(
        script("|export const a = 1\nexport function f() {\n}\n"),
        "export const a = 1\n|export function f() {\n}\n"
    );
    // A closer of what holds it, with no `;` before.
    assert_eq!(
        script("class A {\n  |const f = () => 1\n}\n"),
        "class A {\n  const f = () => 1\n|}\n"
    );
    assert_eq!(
        go(Table::Fundamental, "|(defun a ()\n  (b))\n(c)\n", END, 1),
        "(defun a ()\n  (b))\n|(c)\n"
    );
    assert_eq!(
        go(
            Table::Swift,
            "public final class A {\n    |func b() {\n    }\n}\n",
            END,
            1
        ),
        "public final class A {\n    func b() {\n    }\n|}\n"
    );
    assert_eq!(
        go(
            Table::Css,
            "a {\n  b: c;\n}\n@media x {\n  |d {\n    e: f;\n  }\n}\n",
            END,
            1
        ),
        "a {\n  b: c;\n}\n@media x {\n  d {\n    e: f;\n  }\n|}\n"
    );
}

#[test]
fn bounds_take_in_the_comment_and_attributes_just_above() {
    assert_eq!(
        bounds(
            Table::Rust,
            "fn a() {}\n\n// One.\n// Two.\n#[inline]\nfn b() {\n|}\nx"
        ),
        "fn a() {}\n\n[// One.\n// Two.\n#[inline]\nfn b() {\n}\n]x"
    );
    assert_eq!(
        bounds(Table::Rust, "// Apart.\n\nfn b() {\n|}\n"),
        "// Apart.\n\n[fn b() {\n}\n]"
    );
    // In the comment, before the head, and between defuns: the one that follows.
    assert_eq!(
        bounds(Table::Rust, "fn a() {}\n\n// O|ne.\nfn b() {}\n"),
        "fn a() {}\n\n[// One.\nfn b() {}\n]"
    );
    assert_eq!(
        bounds(Table::Rust, "fn a() {}\n|\nfn b() {}\n"),
        "fn a() {}\n\n[fn b() {}\n]"
    );
    // Past the last, the last; with none, nothing.
    assert_eq!(bounds(Table::Rust, "fn a() {}\n\n|x"), "[fn a() {}\n]\nx");
    assert_eq!(bounds(Table::Rust, "let a = |1;\n"), "None");
    // Nested: the innermost, and between methods the next.
    let at = |find, after| bounds(Table::Rust, &rust_at(find, after));
    assert!(at("let s", 0).contains("[    pub fn one(&self) {\n"));
    assert!(at("let s", 0).contains("// }\n    }\n]"));
    assert!(at("\n    // Two", 1).contains("[    // Two.\n    fn two() {}\n]"));
    assert!(at("[1, 2]", 0).contains("[    const C: [u8; 2] = [1, 2];\n]"));
}

#[test]
fn strings_and_comments_hide_heads_where_highlights_say() {
    let text = "const A: &str = r\"\nfn inside() {}\n\";\nfn b() {\n}\n";
    let string = "r\"\nfn inside() {}\n\"";
    let parts = highlights(
        text,
        &[(text.find(string).unwrap(), string, Highlight::String)],
    );
    let case = format!("{text}|");
    assert_eq!(
        go_in(Table::Rust, &case, Some(&parts), BEGIN, 2),
        format!("|{text}")
    );
    // With no highlights, the line in the string is a head.
    assert!(go(Table::Rust, &case, BEGIN, 2).contains("|fn inside"));
    // A JSON key highlighted as a string is still a head; one in a comment is not.
    let text = "{\n  \"a\": 1,\n  /*\n  \"b\": 2,\n  */\n  \"c\": 3\n}\n";
    let comment = "/*\n  \"b\": 2,\n  */";
    let parts = highlights(
        text,
        &[
            (4, "\"a\"", Highlight::String),
            (text.find(comment).unwrap(), comment, Highlight::Comment),
        ],
    );
    assert_eq!(
        go_in(Table::Json, &format!("{text}|"), Some(&parts), BEGIN, 2),
        format!("{{\n  |{}", &text[4..])
    );
    // In Markdown, a heading's marker is highlighted; a `#` in a fence is not.
    let text = "# A\n```sh\n# not\n```\n";
    let parts = highlights(text, &[(0, "#", Highlight::Markup)]);
    assert_eq!(
        go_in(Table::Markdown, &format!("{text}|"), Some(&parts), BEGIN, 1),
        format!("|{text}")
    );
}

#[test]
fn sections_end_before_the_next_of_their_level_or_above() {
    let md = "# A\ntext\n## B\nmore\n\n# C\n";
    assert_eq!(
        bounds(Table::Markdown, &md.replace("more", "mo|re")),
        "# A\ntext\n[## B\nmore\n]\n# C\n"
    );
    assert_eq!(
        bounds(Table::Markdown, &md.replace("text", "t|ext")),
        "[# A\ntext\n## B\nmore\n]\n# C\n"
    );
    assert_eq!(
        go(Table::Markdown, &format!("|{md}"), END, 1),
        "# A\ntext\n## B\nmore\n|\n# C\n"
    );
    assert_eq!(
        go(Table::Markdown, &format!("{md}|"), BEGIN, 2),
        "# A\ntext\n|## B\nmore\n\n# C\n"
    );
    let toml = "a = 1\n[x]\nb = 2\n\n# C.\n[y]\nc = 3\n";
    assert_eq!(
        bounds(Table::Toml, &toml.replace("b =", "b| =")),
        "a = 1\n[[x]\nb = 2\n]\n# C.\n[y]\nc = 3\n"
    );
    assert_eq!(
        bounds(Table::Toml, &toml.replace("c =", "|c =")),
        "a = 1\n[x]\nb = 2\n\n[# C.\n[y]\nc = 3\n]"
    );
    assert_eq!(
        go(Table::Yaml, "a: 1\nb:\n  c: 2\n  - d\n|", BEGIN, 1),
        "a: 1\n|b:\n  c: 2\n  - d\n"
    );
    let latex = "\\section{A}\nx\n\\subsection{B}\ny\n\\section{C}\n";
    assert_eq!(
        bounds(Table::Latex, &latex.replace("y\n", "|y\n")),
        "\\section{A}\nx\n[\\subsection{B}\ny\n]\\section{C}\n"
    );
    assert_eq!(
        bounds(Table::Latex, &latex.replace("x\n", "|x\n")),
        "[\\section{A}\nx\n\\subsection{B}\ny\n]\\section{C}\n"
    );
    assert_eq!(
        bounds(
            Table::Latex,
            "\\begin{a}\n\\begin{a}\nx\n\\end{a}\n|y\n\\end{a} % Done.\nz\n"
        ),
        "[\\begin{a}\n\\begin{a}\nx\n\\end{a}\ny\n\\end{a} % Done.\n]z\n"
    );
}

#[test]
fn json_keys_at_the_top_level_end_after_their_values() {
    let json = "{\n  \"a\": 1,\n  \"b\": {\n    \"c\": [\n      2\n    ]\n  }\n}\n";
    assert_eq!(
        bounds(Table::Json, &json.replace("\"c\"", "|\"c\"")),
        "{\n  \"a\": 1,\n[  \"b\": {\n    \"c\": [\n      2\n    ]\n  }\n]}\n"
    );
    assert_eq!(
        go(Table::Json, &format!("|{json}"), END, 1),
        "{\n  \"a\": 1,\n|  \"b\": {\n    \"c\": [\n      2\n    ]\n  }\n}\n"
    );
}

#[test]
fn cursors_land_on_clusters_and_text_past_ascii_reads() {
    let text = "// 説明。\nfn 名前() {\n    \"か\u{3099}\"\n}\n";
    assert_eq!(
        go(Table::Rust, &format!("{text}|"), BEGIN, 1),
        "// 説明。\n|fn 名前() {\n    \"か\u{3099}\"\n}\n"
    );
    assert_eq!(
        bounds(Table::Rust, &format!("{text}|")),
        format!("[{text}]")
    );
}

#[test]
fn crafted_text_stays_within_the_budget() {
    // 1 MiB of lines with no head: the walk back reads them all once.
    let lines = "x\n".repeat(1 << 19);
    let display = display(&lines);
    let syntax = Syntax::new(&display, None, Table::Rust);
    let mut selection = Selection::at(lines.len());
    let mut budget = WalkBudget::new();
    assert_eq!(
        beginning_of_defun(&syntax, &mut selection, 1, &mut budget),
        Ok(())
    );
    assert_eq!(selection.cursor, 0);
    // A head whose brace never closes, past the budget: the cursor stays and the budget
    // says why.
    let open = format!("fn a() {{\n{}", "(".repeat(5 << 20));
    let display = self::display(&open);
    let syntax = Syntax::new(&display, None, Table::Rust);
    let mut selection = Selection::at(0);
    let mut budget = WalkBudget::new();
    let _ = end_of_defun(&syntax, &mut selection, 1, &mut budget);
    assert!(budget.is_spent());
    assert_eq!(selection.cursor, 0);
    let mut budget = WalkBudget::new();
    assert_eq!(defun_bounds(&syntax, 3, &mut budget), None);
    assert!(budget.is_spent());
    // A long line with no line break is read only once.
    let long = format!("fn a() {{}}\n{}", "x".repeat(4 << 20));
    let display = self::display(&long);
    let syntax = Syntax::new(&display, None, Table::Rust);
    let mut selection = Selection::at(long.len());
    let mut budget = WalkBudget::new();
    let _ = beginning_of_defun(&syntax, &mut selection, 1, &mut budget);
    assert!(selection.cursor == 0 || budget.is_spent());
}

fn rustish() -> impl Strategy<Value = String> {
    let parts = prop::sample::select(vec![
        "fn a() ",
        "impl X ",
        "struct S;",
        "mod m;",
        "{",
        "}",
        "(",
        ")",
        ";",
        "\n",
        "\n\n",
        "    ",
        "\"",
        "// c",
        "/* ",
        " */",
        "#[x]",
        "x",
        "é",
        "か\u{3099}",
        "\r\n",
    ]);
    prop::collection::vec(parts, 0..40).prop_map(|parts| parts.concat())
}

proptest! {
    #[test]
    fn motions_stop_on_cluster_starts_and_bounds_hold_their_start(
        text in rustish(),
        at in 0usize..200,
        n in -3isize..4,
        highlighted in any::<bool>(),
    ) {
        let display = display(&text);
        let marked = highlights(&text, &[]);
        let syntax = Syntax::new(
            &display,
            highlighted.then_some(&marked),
            Table::Rust,
        );
        let at = crate::motion::cluster_start(&display, at.min(text.len()));
        for (motion, back) in [(BEGIN, true), (END, false)] {
            let mut selection = Selection::at(at);
            let mut budget = WalkBudget::new();
            let _ = motion(&syntax, &mut selection, n, &mut budget);
            let to = selection.cursor;
            prop_assert_eq!(crate::motion::cluster_start(&display, to), to);
            if n > 0 && back {
                prop_assert!(to <= at);
            }
        }
        let mut budget = WalkBudget::new();
        if let Some(range) = defun_bounds(&syntax, at, &mut budget) {
            prop_assert!(range.start < range.end && range.end <= text.len());
            prop_assert!(range.start == 0 || text.as_bytes()[range.start - 1] == b'\n');
            prop_assert!(
                range.end == text.len() || text.as_bytes()[range.end - 1] == b'\n'
            );
        }
    }
}
