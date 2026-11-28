//! Tests of expression motion: cases for each mode, with highlights and without, and
//! properties over balanced and arbitrary text.
//!
//! Must not depend on: `window` or `word`.

use proptest::prelude::*;

use super::*;
use crate::display_map::{DisplayMap, TabWidth};
use crate::rope::Rope;
use crate::syntax::{Highlight, Splice};

fn display(text: &str) -> DisplaySnapshot {
    DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot()
}

/// Highlights with every place each of `parts` is found given its highlight.
fn highlights(text: &str, parts: &[(&str, Highlight)]) -> Highlights {
    let mut highlights = Highlights::new(text.len());
    for (part, highlight) in parts {
        for (at, _) in text.match_indices(part) {
            highlights.replace(at..at + part.len(), [(part.len(), Some(*highlight))]);
        }
    }
    highlights
}

type Motion = fn(&Syntax<'_>, &mut Selection, isize, &mut WalkBudget) -> Result<(), ScanError>;

/// Runs `motion` from the `|` in `case` and shows where it went, or its error and that the
/// cursor stayed.
fn run_in(
    table: Table,
    case: &str,
    parts: Option<&[(&str, Highlight)]>,
    motion: Motion,
    n: isize,
) -> String {
    let from = case.find('|').expect("a | marks point");
    let text = case.replacen('|', "", 1);
    let display = display(&text);
    let highlights = parts.map(|parts| highlights(&text, parts));
    let syntax = Syntax::new(&display, highlights.as_ref(), table);
    let mut selection = Selection::at(from);
    let mut budget = WalkBudget::new();
    match motion(&syntax, &mut selection, n, &mut budget) {
        Ok(()) => format!(
            "{}|{}",
            &text[..selection.cursor],
            &text[selection.cursor..]
        ),
        Err(error) => {
            assert_eq!(selection.cursor, from, "a failed motion stays");
            format!("{error:?}")
        }
    }
}

fn plain(case: &str, motion: Motion, n: isize) -> String {
    run_in(Table::Fundamental, case, None, motion, n)
}

const STRING: Highlight = Highlight::String;
const COMMENT: Highlight = Highlight::Comment;

#[test]
fn a_sexp_is_a_symbol_run_or_a_bracket_group() {
    assert_eq!(
        plain("|foo (bar baz) qux", forward_sexp, 1),
        "foo| (bar baz) qux"
    );
    assert_eq!(
        plain("|foo (bar baz) qux", forward_sexp, 2),
        "foo (bar baz)| qux"
    );
    assert_eq!(
        plain("foo (bar baz) qux|", forward_sexp, -2),
        "foo |(bar baz) qux"
    );
    assert_eq!(
        plain("foo (bar [b {c}]) |qux", forward_sexp, -1),
        "foo |(bar [b {c}]) qux"
    );
    assert_eq!(plain("|  ,. foo_bar1", forward_sexp, 1), "  ,. foo_bar1|");
    assert_eq!(plain("|a b", forward_sexp, 0), "|a b");
}

#[test]
fn past_the_last_sexp_is_the_buffer_edge() {
    assert_eq!(plain("a b|  ", forward_sexp, 1), "a b  |");
    assert_eq!(plain("a b|  ", forward_sexp, 9), "a b  |");
    assert_eq!(plain("  |a", forward_sexp, -2), "|  a");
}

#[test]
fn a_closer_first_or_a_missing_one_is_an_error() {
    assert_eq!(plain("(a|) b", forward_sexp, 1), "EndsPrematurely");
    assert_eq!(plain("a (|b", forward_sexp, -2), "EndsPrematurely");
    assert_eq!(plain("|(a b", forward_sexp, 1), "Unbalanced");
    assert_eq!(plain("a b)|", forward_sexp, -1), "Unbalanced");
    // Any closer closes any opener.
    assert_eq!(plain("|(a] b", forward_sexp, 1), "(a]| b");
}

#[test]
fn strings_and_comments_come_from_the_highlights() {
    let parts: &[(&str, Highlight)] = &[("\"a)\"", STRING), ("/* ( */", COMMENT)];
    let rust = |case, motion, n| run_in(Table::Rust, case, Some(parts), motion, n);
    assert_eq!(rust("|(\"a)\" b) c", forward_sexp, 1), "(\"a)\" b)| c");
    assert_eq!(rust("(\"a)\" b) c|", forward_sexp, -2), "|(\"a)\" b) c");
    assert_eq!(rust("|\"a)\" b", forward_sexp, 1), "\"a)\"| b");
    assert_eq!(rust("|x /* ( */ y", forward_sexp, 2), "x /* ( */ y|");
    assert_eq!(rust("x /* ( */ y|", forward_sexp, -2), "|x /* ( */ y");
    // The same text with no highlights in Fundamental has no strings or comments.
    assert_eq!(plain("|(\"a)\" b) c", forward_sexp, 1), "(\"a)|\" b) c");
}

#[test]
fn with_no_highlights_each_line_is_lexed_with_its_mode() {
    let swift = |case, motion, n| run_in(Table::Swift, case, None, motion, n);
    assert_eq!(swift("|(\"a)\" b) c", forward_sexp, 1), "(\"a)\" b)| c");
    assert_eq!(swift("(\"a)\" b) c|", forward_sexp, -2), "|(\"a)\" b) c");
    assert_eq!(swift("|(a // )\n b)", forward_sexp, 1), "(a // )\n b)|");
    assert_eq!(swift("(a // )\n b)|", forward_sexp, -1), "|(a // )\n b)");
    assert_eq!(
        swift("(a\n\n\"(\" b)|", forward_sexp, -1),
        "|(a\n\n\"(\" b)"
    );
    let latex = |case, motion, n| run_in(Table::Latex, case, None, motion, n);
    assert_eq!(latex("|{a % }\n}", forward_sexp, 1), "{a % }\n}|");
    // A string that goes past its line is not seen.
    assert_eq!(swift("|(\"\n)\" b)", forward_sexp, 1), "(\"\n)|\" b)");
}

#[test]
fn inside_a_string_or_a_comment_a_motion_stays_in_it() {
    let parts: &[(&str, Highlight)] = &[("\"a (b c\"", STRING), ("// x y)", COMMENT)];
    let rust = |case, motion, n| run_in(Table::Rust, case, Some(parts), motion, n);
    assert_eq!(rust("(\"a |(b c\")", forward_sexp, 1), "Unbalanced");
    assert_eq!(rust("(\"a (b |c\")", forward_sexp, 1), "(\"a (b c|\")");
    assert_eq!(rust("(\"a (b c|\")", forward_sexp, 1), "EndsPrematurely");
    assert_eq!(rust("(\"|a (b c\")", forward_sexp, -1), "EndsPrematurely");
    assert_eq!(rust("a // x |y)", forward_sexp, 1), "a // x y|)");
    assert_eq!(rust("a // x y|)", forward_sexp, 1), "EndsPrematurely");
}

#[test]
fn up_list_leaves_a_string_first() {
    let parts: &[(&str, Highlight)] = &[("\"a b\"", STRING)];
    let rust = |case, motion, n| run_in(Table::Rust, case, Some(parts), motion, n);
    assert_eq!(rust("(x \"a |b\")", up_list, -1), "(x |\"a b\")");
    assert_eq!(rust("(x \"a |b\")", up_list, 1), "(x \"a b\"|)");
    assert_eq!(rust("(x \"a |b\")", up_list, -2), "|(x \"a b\")");
    assert_eq!(rust("(x \"a |b\")", up_list, 2), "(x \"a b\")|");
    assert_eq!(rust("(x \"a |b\")", up_list, 3), "Unbalanced");
    assert_eq!(plain("a |b", up_list, -1), "Unbalanced");
    assert_eq!(plain("[a (b |c) d]", up_list, -2), "|[a (b c) d]");
}

#[test]
fn down_list_goes_into_the_next_list() {
    assert_eq!(plain("|a (b c)", down_list, 1), "a (|b c)");
    assert_eq!(plain("|a (b [c])", down_list, 2), "a (b [|c])");
    assert_eq!(plain("(b c) a|", down_list, -1), "(b c|) a");
    assert_eq!(plain("(a|) (b)", down_list, 1), "EndsPrematurely");
    assert_eq!(plain("|a b", down_list, 1), "Unbalanced");
}

#[test]
fn forward_list_passes_what_is_no_list() {
    assert_eq!(plain("|a (b) c (d) e", forward_list, 1), "a (b)| c (d) e");
    assert_eq!(plain("|a (b) c (d) e", forward_list, 2), "a (b) c (d)| e");
    assert_eq!(plain("|a (b) c (d) e", forward_list, 3), "a (b) c (d) e|");
    assert_eq!(plain("a (b) c (d) e|", forward_list, -1), "a (b) c |(d) e");
    assert_eq!(plain("(a |b) c", forward_list, 1), "EndsPrematurely");
}

#[test]
fn symbols_are_letters_and_digits_of_any_script() {
    assert_eq!(plain("|日本語の文章 x", forward_sexp, 1), "日本語の文章| x");
    assert_eq!(plain("|日本語、文章", forward_sexp, 1), "日本語|、文章");
    assert_eq!(plain("日本語、文章|", forward_sexp, -1), "日本語、|文章");
    let script = |case| run_in(Table::Script, case, None, forward_sexp, 1);
    assert_eq!(script("|$foo.bar"), "$foo|.bar");
    let css = |case| run_in(Table::Css, case, None, forward_sexp, 1);
    assert_eq!(css("|margin-top: 0"), "margin-top|: 0");
    let rust = |case| run_in(Table::Rust, case, None, forward_sexp, 1);
    assert_eq!(rust("|foo-bar"), "foo|-bar");
}

#[test]
fn a_mark_goes_with_what_it_follows() {
    // `e` and a combining acute accent, and a `)` with one.
    assert_eq!(plain("|cafe\u{301}s x", forward_sexp, 1), "cafe\u{301}s| x");
    assert_eq!(
        plain("cafe\u{301}s| x", forward_sexp, -1),
        "|cafe\u{301}s x"
    );
    assert_eq!(plain("|(a)\u{301} b", forward_sexp, 1), "(a)\u{301}| b");
    assert_eq!(plain("(a)\u{301}| b", forward_sexp, -1), "|(a)\u{301} b");
    assert_eq!(plain("a)\u{301}| b", up_list, -1), "Unbalanced");
}

#[test]
fn an_html_tag_is_a_bracket_pair() {
    let parts: &[(&str, Highlight)] = &[("\"a>\"", STRING), ("<!-- < -->", COMMENT)];
    let html = |case, motion, n| run_in(Table::Html, case, Some(parts), motion, n);
    assert_eq!(
        html("|<div class=\"a>\">x</div>", forward_sexp, 1),
        "<div class=\"a>\">|x</div>"
    );
    assert_eq!(
        html("<div class=\"a>\">|x</div>", forward_sexp, 2),
        "<div class=\"a>\">x</div>|"
    );
    assert_eq!(
        html("<div class=\"a>\">x</div>|", forward_sexp, -1),
        "<div class=\"a>\">x|</div>"
    );
    assert_eq!(
        html("|<!-- < --> <br>", forward_sexp, 1),
        "<!-- < --> <br>|"
    );
    assert_eq!(html("|a < b", forward_sexp, 2), "a < b|");
    assert_eq!(html("a < b|", forward_sexp, -2), "|a < b");
    // Outside HTML, `<` and `>` are punctuation.
    assert_eq!(plain("|Vec<T> x", forward_sexp, 2), "Vec<T|> x");
}

#[test]
fn highlights_after_an_edit_guess_until_the_next_report() {
    let before = "x \"ab\" y";
    let mut guessed = highlights(before, &[("\"ab\"", STRING)]);
    // `)` typed after `a` takes the string's highlight.
    guessed.splice(&[Splice {
        start: 4,
        old_len: 0,
        new_len: 1,
    }]);
    let text = "x \"a)b\" y";
    let display = display(text);
    let syntax = Syntax::new(&display, Some(&guessed), Table::Rust);
    let mut budget = WalkBudget::new();
    assert_eq!(sexp_end(&syntax, 1, 1, &mut budget), Ok(7));
    // Highlights that do not cover the text are not read.
    let short = highlights(before, &[("\"ab\"", STRING)]);
    let syntax = Syntax::new(&display, Some(&short), Table::Fundamental);
    assert_eq!(sexp_end(&syntax, 1, 1, &mut budget), Ok(4));
}

fn transposed(case: &str, n: isize) -> Result<(String, String), ScanError> {
    let from = case.find('|').expect("a | marks point");
    let text = case.replacen('|', "", 1);
    let display = display(&text);
    let syntax = Syntax::new(&display, None, Table::Fundamental);
    let (first, second) = transpose_ranges(&syntax, from, n, &mut WalkBudget::new())?;
    Ok((text[first].to_owned(), text[second].to_owned()))
}

#[test]
fn transpose_finds_the_sexp_before_point_and_those_after() {
    let pair = |a: &str, b: &str| Ok((a.to_owned(), b.to_owned()));
    assert_eq!(transposed("a |b c", 1), pair("a", "b"));
    assert_eq!(transposed("a| b c", 1), pair("a", "b"));
    assert_eq!(transposed("a |b c", 2), pair("a", "b c"));
    assert_eq!(transposed("(a) |[b c] d", 1), pair("(a)", "[b c]"));
    assert_eq!(transposed("a b |c", -1), pair("a", "b"));
    assert_eq!(transposed("a b c |d", -2), pair("a b", "c"));
    assert_eq!(transposed("|a b", 1), Err(ScanError::NothingToTranspose));
    assert_eq!(transposed("a b|", 1), Err(ScanError::NothingToTranspose));
    assert_eq!(transposed("a |b", -1), Err(ScanError::NothingToTranspose));
    assert_eq!(transposed("a |b", 0), Err(ScanError::NothingToTranspose));
}

#[test]
fn a_motion_past_its_budget_stays_and_says_so() {
    let text = format!("({}", "a ".repeat(1000));
    let display = display(&text);
    let syntax = Syntax::new(&display, None, Table::Fundamental);
    let mut selection = Selection::at(0);
    let mut budget = WalkBudget::with_limit(SEEK_LIMIT);
    assert_eq!(
        forward_sexp(&syntax, &mut selection, 1, &mut budget),
        Err(ScanError::TooFar)
    );
    assert_eq!(selection.cursor, 0);
    assert_eq!(ScanError::TooFar.message(), "Count too large");
}

/// A budget that holds a few seeks and a little text.
const SEEK_LIMIT: usize = 4 * crate::motion::SEEK_COST;

#[test]
fn crafted_text_stays_within_a_commands_budget() {
    // A MiB of openers: unbalanced, found by reading it once.
    let text = "(".repeat(1 << 20);
    let shown = display(&text);
    let syntax = Syntax::new(&shown, None, Table::Fundamental);
    let mut budget = WalkBudget::new();
    assert_eq!(
        sexp_end(&syntax, 0, 1, &mut budget),
        Err(ScanError::Unbalanced)
    );
    // Half a million strings, each its own run: each costs what it holds.
    let text = "\"\" ".repeat(1 << 19);
    let shown = display(&text);
    let highlights = highlights(&text, &[("\"\"", STRING)]);
    let syntax = Syntax::new(&shown, Some(&highlights), Table::Rust);
    let mut selection = Selection::at(0);
    let mut budget = WalkBudget::new();
    assert_eq!(
        forward_list(&syntax, &mut selection, 1, &mut budget),
        Ok(())
    );
    assert_eq!(selection.cursor, text.len());
    // More than a command reads: it stops, and the cursor stays.
    let text = format!("({}", "x ".repeat(3 << 20));
    let shown = display(&text);
    let syntax = Syntax::new(&shown, None, Table::Rust);
    let mut budget = WalkBudget::new();
    assert_eq!(sexp_end(&syntax, 0, 1, &mut budget), Err(ScanError::TooFar));
    // A long line with no highlights is all code past what is lexed.
    let text = format!("(\"){}\")", "y".repeat(lex::LINE_MAX));
    let shown = display(&text);
    let syntax = Syntax::new(&shown, None, Table::Rust);
    assert_eq!(sexp_end(&syntax, 0, 1, &mut WalkBudget::new()), Ok(3));
}

#[test]
fn a_table_for_each_mode() {
    assert_eq!(Table::of(Mode::Rust), Table::Rust);
    assert_eq!(Table::of(Mode::Tsx), Table::Script);
    assert_eq!(Table::of(Mode::Plan), Table::Markdown);
    assert_eq!(Table::of(Mode::Review), Table::Fundamental);
}

/// An expression, for the properties below.
#[derive(Clone, Debug)]
enum Sexp {
    Atom(&'static str),
    Str,
    List(usize, Vec<Sexp>),
}

fn sexp() -> impl Strategy<Value = Sexp> {
    let leaf = prop_oneof![
        prop::sample::select(vec!["foo", "x1", "a_b", "日本", "e\u{301}"]).prop_map(Sexp::Atom),
        Just(Sexp::Str),
    ];
    leaf.prop_recursive(4, 40, 5, |inner| {
        (0..3usize, prop::collection::vec(inner, 0..5))
            .prop_map(|(kind, items)| Sexp::List(kind, items))
    })
}

/// Writes `sexp` to `text`, a string's range to `strings` and a comment's to `comments`.
fn write(
    sexp: &Sexp,
    text: &mut String,
    strings: &mut Vec<Range<usize>>,
    comments: &mut Vec<Range<usize>>,
) {
    match sexp {
        Sexp::Atom(atom) => text.push_str(atom),
        Sexp::Str => {
            let start = text.len();
            text.push_str("\"a)(\"");
            strings.push(start..text.len());
        }
        Sexp::List(kind, items) => {
            let (open, close) = [("(", ")"), ("[", "]"), ("{", "}")][*kind];
            text.push_str(open);
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    text.push_str(if i % 3 == 0 { "\n" } else { " " });
                }
                if i % 2 == 1 {
                    let start = text.len();
                    text.push_str("/* ) */");
                    comments.push(start..text.len());
                    text.push(' ');
                }
                write(item, text, strings, comments);
            }
            text.push_str(close);
        }
    }
}

/// The text of `sexps` side by side, each one's range, and highlights for its strings and
/// comments.
fn written(sexps: &[Sexp]) -> (String, Vec<Range<usize>>, Highlights) {
    let (mut text, mut strings, mut comments, mut ranges) = (String::new(), vec![], vec![], vec![]);
    for sexp in sexps {
        text.push(' ');
        let start = text.len();
        write(sexp, &mut text, &mut strings, &mut comments);
        ranges.push(start..text.len());
    }
    text.push(' ');
    let mut highlights = Highlights::new(text.len());
    for range in strings {
        highlights.replace(range.clone(), [(range.len(), Some(STRING))]);
    }
    for range in comments {
        highlights.replace(range.clone(), [(range.len(), Some(COMMENT))]);
    }
    (text, ranges, highlights)
}

/// A piece of arbitrary text, brackets, quotes, comment markers and marks among it.
fn piece() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "a", "日", " ", "\n", "(", ")", "[", "]", "{", "}", "<", ">", "\"", "'", "`", "//", "/*",
        "*/", "#", "%", "\\", "\u{301}", "\u{200d}", "👍", "\r\n", "\t", "-", "$",
    ])
}

const TABLES: [Table; 12] = [
    Table::Fundamental,
    Table::Rust,
    Table::Go,
    Table::Script,
    Table::Css,
    Table::Json,
    Table::Html,
    Table::Markdown,
    Table::Toml,
    Table::Yaml,
    Table::Latex,
    Table::Swift,
];

proptest! {
    #[test]
    fn forward_and_backward_over_balanced_text_agree(sexps in prop::collection::vec(sexp(), 1..5)) {
        let (text, ranges, highlights) = written(&sexps);
        let display = display(&text);
        // With highlights, and with each line lexed: the strings and comments are on one line.
        for highlights in [Some(&highlights), None] {
            let syntax = Syntax::new(&display, highlights, Table::Rust);
            for range in &ranges {
                let mut budget = WalkBudget::new();
                prop_assert_eq!(sexp_end(&syntax, range.start, 1, &mut budget), Ok(range.end));
                prop_assert_eq!(sexp_end(&syntax, range.end, -1, &mut budget), Ok(range.start));
                prop_assert_eq!(sexp_end(&syntax, range.start - 1, 1, &mut budget), Ok(range.end));
                if text[range.clone()].starts_with(['(', '[', '{']) {
                    let mut selection = Selection::at(range.start);
                    prop_assert_eq!(down_list(&syntax, &mut selection, 1, &mut budget), Ok(()));
                    prop_assert_eq!(up_list(&syntax, &mut selection, -1, &mut budget), Ok(()));
                    prop_assert_eq!(selection.cursor, range.start);
                    let mut selection = Selection::at(range.start + 1);
                    prop_assert_eq!(up_list(&syntax, &mut selection, 1, &mut budget), Ok(()));
                    prop_assert_eq!(selection.cursor, range.end);
                }
            }
            let mut budget = WalkBudget::new();
            let count = isize::try_from(ranges.len()).unwrap_or(isize::MAX);
            prop_assert_eq!(sexp_end(&syntax, 0, count, &mut budget), Ok(ranges[ranges.len() - 1].end));
            prop_assert_eq!(sexp_end(&syntax, text.len(), -count, &mut budget), Ok(ranges[0].start));
        }
    }

    #[test]
    fn any_text_scans_to_a_clusters_start_and_never_panics(
        pieces in prop::collection::vec(piece(), 0..40),
        at in any::<prop::sample::Index>(),
        n in -3isize..=3,
    ) {
        let text = pieces.concat();
        let display = display(&text);
        let starts: Vec<usize> = (0..=text.len())
            .filter(|&offset| display.cluster_at(offset).0 == offset)
            .collect();
        let from = starts[at.index(starts.len())];
        let motions: [Motion; 4] = [forward_sexp, forward_list, up_list, down_list];
        for table in TABLES {
            let syntax = Syntax::new(&display, None, table);
            for motion in motions {
                let mut selection = Selection::at(from);
                if motion(&syntax, &mut selection, n, &mut WalkBudget::new()).is_ok() {
                    prop_assert!(starts.contains(&selection.cursor), "{table:?} to {}", selection.cursor);
                } else {
                    prop_assert_eq!(selection.cursor, from);
                }
            }
            if let Ok((first, second)) = transpose_ranges(&syntax, from, n, &mut WalkBudget::new()) {
                prop_assert!(first.end <= second.start);
                prop_assert!(starts.contains(&first.start) && starts.contains(&second.end));
            }
        }
    }
}
