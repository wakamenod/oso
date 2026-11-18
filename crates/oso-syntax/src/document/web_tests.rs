//! Tests of HTML, CSS, JSON and YAML (ADR 0157): each one's captures, HTML's embedded code and
//! which `type` names it, its cap and edits that change it, their fences, and a YAML tree that
//! ends before its text. Crafted nesting is `crafted`'s.
//!
//! Must not depend on: the worker; these call a document directly.

use oso_core::rope::Rope;
use oso_core::syntax::Highlight::{
    self, Comment, Constant, Function, Keyword, Macro, Property, String, Type,
};
use oso_core::syntax::Splice;

use super::embedded::MAX_EMBEDDED_BYTES;
use super::highlight_tests::{highlighted, runs};
use super::tests::LONG;
use super::{Document, Highlighted};
use crate::language::Language;
use crate::queries::Queries;

type Runs = Vec<(std::string::String, Highlight)>;

fn has(runs: &Runs, text: &str, highlight: Highlight) -> bool {
    runs.iter().any(|(run, h)| run == text && *h == highlight)
}

#[track_caller]
fn assert_runs(runs: &Runs, expected: &[(&str, Highlight)]) {
    for (part, highlight) in expected {
        assert!(
            has(runs, part, *highlight),
            "{part:?} as {highlight:?} in {runs:?}"
        );
    }
}

#[track_caller]
fn assert_has(language: Language, text: &str, expected: &[(&str, Highlight)]) {
    assert_runs(&runs(&highlighted(language, text)), expected);
}

/// No run holds `part`, whatever it is.
#[track_caller]
fn assert_plain(runs: &Runs, part: &str) {
    assert!(
        runs.iter().all(|(run, _)| !run.contains(part)),
        "{part:?} in {runs:?}"
    );
}

#[test]
fn html_captures() {
    let text = "<!DOCTYPE html>\n<html lang=\"en\">\n<!-- a <b> \"c\" -->\n\
                <p class=a id='b' hidden>A &amp; B &#169;</p>\n<br/>\n</div>\n</html>\n";
    let runs = runs(&highlighted(Language::Html, text));
    assert_runs(
        &runs,
        &[
            ("<!DOCTYPE html>", Keyword),
            ("html", Function),
            ("lang", Property),
            ("\"en\"", String),
            ("<!-- a <b> \"c\" -->", Comment),
            ("p", Function),
            ("class", Property),
            ("a", String),
            ("'b'", String),
            ("hidden", Property),
            ("&amp;", Constant),
            ("&#169;", Constant),
            ("br", Function),
        ],
    );
    // Text, angle brackets and an end tag that closes nothing stay plain.
    assert_plain(&runs, "A ");
    assert_plain(&runs, "div");
    assert!(
        runs.iter()
            .all(|(run, _)| !run.starts_with(['<', '>']) || run.starts_with("<!")),
        "{runs:?}"
    );
}

#[test]
fn css_captures() {
    let text = "@media (min-width: 600px) {\n  a:hover, .x > #y::before, [href^=\"h\"] {\n    \
                --c: #fff; color: var(--c) !important; margin: 0 1.5em;\n    \
                background: url(a.png); font-family: \"A\", sans-serif;\n  }\n}\n\
                @import \"x.css\";\n/* a { b: c } */\n";
    let runs = runs(&highlighted(Language::Css, text));
    assert_runs(
        &runs,
        &[
            ("@media", Keyword),
            ("min-width", Property),
            ("600px", Constant),
            ("a", Function),
            ("hover", Type),
            ("x", Type),
            ("y", Type),
            ("before", Type),
            ("href", Type),
            ("\"h\"", String),
            ("--c", Property),
            ("#fff", Constant),
            ("color", Property),
            ("var", Function),
            ("!important", Keyword),
            ("0", Constant),
            ("1.5em", Constant),
            ("url", Function),
            ("a.png", String),
            ("\"A\"", String),
            ("@import", Keyword),
            ("\"x.css\"", String),
            ("/* a { b: c } */", Comment),
        ],
    );
    assert_plain(&runs, "sans-serif");
}

#[test]
fn json_captures() {
    let text = "{\n  // a \"b\" 1\n  \"a\": [1, -2.5e3, true, null, \"s\\n\"],\n  \
                \"b\": {\"c\": false}\n}\n";
    assert_has(
        Language::Json,
        text,
        &[
            ("// a \"b\" 1", Comment),
            ("\"a\"", Property),
            ("1", Constant),
            ("-2.5e3", Constant),
            ("true", Constant),
            ("null", Constant),
            ("\"s\\n\"", String),
            ("\"c\"", Property),
            ("false", Constant),
        ],
    );
}

#[test]
fn yaml_captures() {
    let text = "%YAML 1.2\n---\n# a: \"b\" 1\nkey: value\n\"q\": 's'\n1: one\nlist:\n  - 2\n  \
                - 2.5\n  - true\n  - ~\nanchor: &a {x: 1, \"y\": [a, b]}\nref: *a\n\
                tagged: !!str 12\nblock: |\n  text\n...\n";
    assert_has(
        Language::Yaml,
        text,
        &[
            ("%YAML 1.2", Keyword),
            ("---", Keyword),
            ("# a: \"b\" 1", Comment),
            ("key", Property),
            ("value", String),
            ("\"q\"", Property),
            ("'s'", String),
            // A key is a key, a number or not.
            ("1", Property),
            ("one", String),
            ("2", Constant),
            ("2.5", Constant),
            ("true", Constant),
            ("~", Constant),
            ("&a", Macro),
            ("x", Property),
            ("\"y\"", Property),
            ("a", String),
            ("*a", Macro),
            ("!!str", Type),
            ("12", Constant),
            ("|", String),
            ("  text", String),
            ("...", Keyword),
        ],
    );
}

/// `C-e` takes a comment's run for the whole comment (ADR 0111, step 6), so no capture may
/// paint inside one.
#[test]
fn a_comment_is_one_run() {
    assert_has(
        Language::Html,
        "<p>x</p> <!-- <a href=\"b\"> &amp; -->\n",
        &[("<!-- <a href=\"b\"> &amp; -->", Comment)],
    );
    assert_has(
        Language::Css,
        "a { b: c; } /* \"s\" 1px url(x) @media */\n",
        &[("/* \"s\" 1px url(x) @media */", Comment)],
    );
    assert_has(
        Language::Json,
        "{\"a\": 1} // \"b\": true\n",
        &[("// \"b\": true", Comment)],
    );
    assert_has(
        Language::Yaml,
        "a: 1 # \"b\": true &c\n",
        &[("# \"b\": true &c", Comment)],
    );
}

#[test]
fn line_breaks_are_never_highlighted() {
    let cases = [
        (Language::Html, "<!-- a\nb -->\n<p title=\"a\nb\">x</p>\n"),
        (Language::Css, "/* a\nb */\na { content: \"x\" }\n"),
        (Language::Json, "{\"a\":\n\"b\"}\n"),
        (Language::Yaml, "a: |\n  b\n  c\nd: \"e\n  f\"\n"),
        (
            Language::Html,
            "<script>\nlet s = `a\nb`;\n</script>\n<style>\n/* a\nb */\n</style>\n",
        ),
    ];
    for (language, text) in cases {
        let document = highlighted(language, text);
        let highlights = document.highlights();
        for (at, _) in text.match_indices('\n') {
            assert_eq!(highlights.at(at), None, "{language:?} at {at}");
        }
        assert!(!runs(&document).is_empty(), "{language:?}");
    }
}

#[test]
fn a_script_is_javascript_and_a_style_css() {
    let text = "<p>x</p>\n<script>\nconst n = f(1); // c\n</script>\n\
                <style>\np { color: red; }\n</style>\n";
    let runs = runs(&highlighted(Language::Html, text));
    assert_runs(
        &runs,
        &[
            ("script", Function),
            ("const", Keyword),
            ("f", Function),
            ("1", Constant),
            ("// c", Comment),
            ("style", Function),
            ("p", Function),
            ("color", Property),
        ],
    );
}

/// The first `type` names the language, trimmed and ignoring ASCII case; one oso does not
/// know leaves the element plain.
#[test]
fn a_scripts_type_names_its_language() {
    let javascript = [
        "<script type=module>let a = 1;</script>",
        "<script type=\"\">let a = 1;</script>",
        "<script type>let a = 1;</script>",
        "<script type=\" Text/JavaScript \">let a = 1;</script>",
        "<script async type=\"text/babel\" type=\"text/template\">let a = 1;</script>",
    ];
    for text in javascript {
        let runs = runs(&highlighted(Language::Html, text));
        assert_runs(&runs, &[("let", Keyword), ("1", Constant)]);
    }
    let json = [
        "<script type=\"importmap\">{\"a\": 1}</script>",
        "<script type=\"application/ld+json\">{\"a\": 1}</script>",
    ];
    for text in json {
        let runs = runs(&highlighted(Language::Html, text));
        assert_runs(&runs, &[("\"a\"", Property), ("1", Constant)]);
    }
    let plain = [
        "<script type=\"text/template\">let a = 1;</script>",
        "<script type=\"text/javascript-but-a-name-far-too-long-to-be-one\">let a = 1;</script>",
        "<style type=\"text/less\">let a = 1;</style>",
    ];
    for text in plain {
        let runs = runs(&highlighted(Language::Html, text));
        assert_plain(&runs, "let");
        assert_plain(&runs, "1");
    }
}

/// An element past the cap stays plain, as a fence past its own does.
#[test]
fn an_element_past_the_cap_stays_plain() {
    let body = "let a = 1;\n".repeat(MAX_EMBEDDED_BYTES / 11 + 1);
    let over = runs(&highlighted(
        Language::Html,
        &format!("<script>{body}</script>\n<p a=b>x</p>\n"),
    ));
    assert_plain(&over, "let");
    assert_runs(&over, &[("p", Function), ("a", Property)]);
    let under = "let a = 1;\n".repeat(MAX_EMBEDDED_BYTES / 11 - 2);
    let under = runs(&highlighted(
        Language::Html,
        &format!("<script>{under}</script>\n"),
    ));
    assert_runs(&under, &[("let", Keyword)]);
}

/// A `type` typed into a start tag takes the element's code away, to its end, though only the
/// tag's line changed; deleting it brings the code back.
#[test]
fn an_edit_to_a_start_tag_highlights_its_element_again() {
    let body = "let a = 1;\n".repeat(400);
    let before = format!("<script>\n{body}</script>\n");
    let mut queries = Queries::new();
    let mut document = Document::new(Language::Html, Rope::from(before.as_str()));
    assert_eq!(
        document.highlight(&mut queries, &[], LONG),
        Highlighted::Done
    );
    let last = before.rfind("let").unwrap();
    assert_eq!(document.highlights().at(last), Some(Keyword));
    let typed = " type=\"text/template\"";
    let after = before.replacen("<script>", &format!("<script{typed}>"), 1);
    let splice = Splice {
        start: "<script".len(),
        old_len: 0,
        new_len: typed.len(),
    };
    document.edit(&[splice], &Rope::from(after.as_str()));
    assert_eq!(
        document.highlight(&mut queries, &[], LONG),
        Highlighted::Done
    );
    let last = after.rfind("let").unwrap();
    assert_eq!(document.highlights().at(last), None);
    let undo = Splice {
        start: "<script".len(),
        old_len: typed.len(),
        new_len: 0,
    };
    document.edit(&[undo], &Rope::from(before.as_str()));
    assert_eq!(
        document.highlight(&mut queries, &[], LONG),
        Highlighted::Done
    );
    let last = before.rfind("let").unwrap();
    assert_eq!(document.highlights().at(last), Some(Keyword));
}

/// HTML's embedded grammars are compiled at their first element, not with HTML's.
#[test]
fn an_embedded_query_is_compiled_at_its_first_element() {
    let mut queries = Queries::new();
    let mut document = Document::new(Language::Html, Rope::from("<p>x</p>\n"));
    assert_eq!(
        document.highlight(&mut queries, &[], LONG),
        Highlighted::Done
    );
    let tried =
        |queries: &Queries, language| queries.tried(crate::queries::Kind::Highlights(language));
    assert!(!tried(&queries, Language::JavaScript));
    assert!(!tried(&queries, Language::Css));
    let text = "<style>a { b: c }</style>\n";
    let mut document = Document::new(Language::Html, Rope::from(text));
    assert_eq!(
        document.highlight(&mut queries, &[], LONG),
        Highlighted::Done
    );
    assert!(tried(&queries, Language::Css));
    assert!(!tried(&queries, Language::JavaScript));
}

#[test]
fn fences_of_html_css_json_and_yaml() {
    let text = "```html\n<p a=\"b\">x</p><script>let c = 1;</script>\n```\n\n\
                ```CSS\na { color: red }\n```\n\n```jsonc\n{\"k\": true}\n```\n\n\
                ```yml\nkey: 1\n```\n\n```xml\n<a b=\"c\"/>\n```\n";
    let runs = runs(&highlighted(Language::Markdown, text));
    assert_runs(
        &runs,
        &[
            ("p", Function),
            ("\"b\"", String),
            ("color", Property),
            ("\"k\"", Property),
            ("true", Constant),
            ("key", Property),
            ("1", Constant),
        ],
    );
    // Code embedded in a fence's code is not parsed again, and `xml` names no grammar: both
    // stay as the fence's text is, plain inside its delimiters.
    assert_plain(&runs, "let");
    assert_plain(&runs, "\"c\"");
}

/// A YAML tree can end at an error before its text does (ADR 0157): what lies past it stays
/// plain, and the document is still highlighted to its end.
#[test]
fn text_past_a_yaml_tree_stays_plain() {
    // As the property test found it: an item's text cut so that the scanner gives up.
    let unit = "# c\nkey: v\nlist:\n  - 1\n  - {x: \"y\", z: [a, b]}\nblock: |\n  text\n";
    let whole = unit.repeat(8);
    let cut = whole.find("  - {x").unwrap() + 2 * unit.len();
    let text = format!("{}{}", &whole[..cut], &whole[cut + 7..]);
    let document = highlighted(Language::Yaml, &text);
    let tree = document.tree().unwrap();
    let end = tree.root_node().end_byte();
    assert!(
        end < text.len() && tree.root_node().has_error(),
        "{end} of {}",
        text.len()
    );
    let highlights = document.highlights();
    assert_eq!(highlights.len(), text.len());
    assert!(highlights.runs(end..text.len()).all(|(_, h)| h.is_none()));
    assert!(document.is_highlighted(std::slice::from_ref(&(0..text.len()))));
}
