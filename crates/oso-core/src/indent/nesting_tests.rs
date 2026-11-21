//! Tests of the rules of the languages of brackets and tags (ADR 0164): text as Prettier and
//! gofmt write it is a fixed point of each, new lines, strings and comments, labels, JSX and
//! HTML's elements, scripts and styles, and the answer past the budget.
//!
//! Must not depend on: `buffer` or `editor`; they run on ropes alone.

use super::pass::measure;
use super::tests::Rules;
use super::{
    Answer, Budget, LINE_COST, LINE_LOOKBACK, Line, Settings, Style, css, go, html, javascript,
    json, typescript,
};
use crate::display_map::TabWidth;
use crate::rope::Rope;

/// Two spaces a level, as Prettier writes.
fn two() -> Settings {
    Settings::new(2, Style::Spaces, 8).unwrap()
}

/// A tab a level, as gofmt writes.
fn tabs() -> Settings {
    Settings::new(8, Style::Tabs, 8).unwrap()
}

/// The non-blank lines of `text` whose answer is a column other than the one they have: their
/// row, the answer and the column as written.
fn misindented(rules: Rules, settings: Settings, text: &str) -> Vec<(usize, usize, usize)> {
    let rope = Rope::from(text);
    let lines: Vec<_> = (0..rope.line_count()).map(Line::Row).collect();
    let needed = rope.len() + lines.len() * LINE_COST + LINE_LOOKBACK;
    let mut budget = Budget::with_limit(needed);
    let answers = rules(&rope, settings, &lines, &mut budget);
    assert!(!budget.is_spent(), "the budget ran out");
    text.split('\n')
        .zip(answers)
        .enumerate()
        .filter_map(|(row, (line, answer))| {
            let written = measure(line, TabWidth::default());
            let Answer::Column(answer) = answer else {
                return None;
            };
            (!written.blank && answer != written.column).then_some((row, answer, written.column))
        })
        .collect()
}

#[track_caller]
fn fixed(rules: Rules, settings: Settings, text: &str) {
    assert_eq!(misindented(rules, settings, text), [], "{text}");
}

/// The answer for a new line at `offset` of `text`.
fn new_line(rules: Rules, settings: Settings, text: &str, offset: usize) -> Answer {
    let rope = Rope::from(text);
    rules(&rope, settings, &[Line::New(offset)], &mut Budget::new())[0]
}

/// The answer for row `row` of `text`.
fn row(rules: Rules, settings: Settings, text: &str, row: usize) -> Answer {
    let rope = Rope::from(text);
    rules(&rope, settings, &[Line::Row(row)], &mut Budget::new())[0]
}

#[test]
fn css_as_prettier_writes_it() {
    fixed(
        css,
        two(),
        "/* A comment\n * on lines\n */\n:root {\n  --accent: #0a84ff;\n}\n\n\
         nav a:hover,\n.button > span::after {\n  color: var(--accent);\n  \
         background: url(\"a{b}.png\");\n}\n\n@media (min-width: 600px) {\n  .card {\n    \
         padding: 0 calc(1rem + 2px);\n  }\n}\n",
    );
}

#[test]
fn json_as_prettier_writes_it_comments_too() {
    fixed(
        json,
        two(),
        "{\n  // a comment with { and [\n  \"name\": \"a } b\",\n  \"list\": [\n    1,\n    \
         {\n      \"deep\": [true, null]\n    }\n  ],\n  /* a\n   * block\n   */\n  \"x\": {}\n}\n",
    );
}

#[test]
fn a_closer_goes_back_and_a_new_line_one_level_in() {
    let text = "{\n  \"a\": [\n";
    assert_eq!(new_line(json, two(), text, text.len()), Answer::Column(4));
    assert_eq!(
        new_line(json, two(), "{\n  \"a\": 1\n}", 13),
        Answer::Column(0)
    );
    let css_text = "a {\n  color: red;\n}\n";
    assert_eq!(row(css, two(), css_text, 2), Answer::Column(0));
}

#[test]
fn typescript_as_prettier_writes_it() {
    fixed(
        typescript,
        two(),
        "import { a } from \"b\";\n\nexport interface Point {\n  x: number;\n  y: number;\n}\n\n\
         export function f(\n  a: number,\n  b: string,\n): Promise<void> {\n  const x =\n    \
         compute(a);\n  const v = items\n    .map((i) => i + 1)\n    .filter(Boolean);\n  \
         if (a > 1 && b) {\n    return;\n  }\n  const s = `a ${b} c`;\n  const r = /[}{]\\//g;\n  \
         return x;\n}\n\ntype U =\n  | { kind: \"a\" }\n  | { kind: \"b\" };\n\n\
         const y = cond\n  ? a\n  : b;\n",
    );
}

#[test]
fn a_switch_s_labels_sit_one_level_in_and_their_lines_two() {
    fixed(
        typescript,
        two(),
        "function f(x: number) {\n  switch (x) {\n    case 1:\n      a();\n      break;\n    \
         case 2: {\n      b();\n      break;\n    }\n    default:\n      c();\n  }\n  \
         const o = {\n    default: 1,\n    other: 2,\n  };\n}\n",
    );
}

#[test]
fn a_template_literal_keeps_its_lines_and_its_code_counts() {
    let text = "const s = `\nkeep\n    this ${f({\n  a: 1,\n})}\n`;\nlet x = 1;\n";
    let rope = Rope::from(text);
    let lines: Vec<_> = (0..rope.line_count()).map(Line::Row).collect();
    let answers = typescript(&rope, two(), &lines, &mut Budget::new());
    assert_eq!(answers[1], Answer::Keep);
    assert_eq!(answers[2], Answer::Keep);
    assert_eq!(answers[3], Answer::Column(6));
    assert_eq!(answers[4], Answer::Column(4));
    assert_eq!(answers[5], Answer::Keep);
    assert_eq!(answers[6], Answer::Column(0));
}

#[test]
fn a_regular_expression_s_brackets_and_quotes_count_for_nothing() {
    fixed(
        typescript,
        two(),
        "function f(s) {\n  const a = s.replace(/[\"'{]/g, \"\");\n  const b = s.split(/\\/(?=\\{)/);\n  \
         const c = 4 / 2 / 1;\n  return a;\n}\n",
    );
}

#[test]
fn jsx_as_prettier_writes_it() {
    fixed(
        javascript,
        two(),
        "export function App({ items }) {\n  return (\n    <div className=\"app\">\n      \
         <Header title=\"Don't { panic\" />\n      <Item\n        key={1}\n        \
         onClick={() => {\n          go();\n        }}\n      />\n      {items.map((i) => (\n        \
         <li key={i}>{i}</li>\n      ))}\n      <p>\n        Don't stop: {name}\n      </p>\n      \
         <>\n        <b>a</b>\n      </>\n    </div>\n  );\n}\n",
    );
}

#[test]
fn go_as_gofmt_writes_it() {
    fixed(
        go,
        tabs(),
        "package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n\nfunc main() {\n\tswitch x := f(); x {\n\t\
         case 1:\n\t\tfmt.Println(`raw {\nstring`)\n\tdefault:\n\t\tos.Exit(1)\n\t}\n\tif a &&\n\t\tb {\n\t\t\
         c := []int{\n\t\t\t1,\n\t\t}\n\t\t_ = c\n\t}\n\tr := '}'\n\t_ = r\n}\n",
    );
}

#[test]
fn html_as_prettier_writes_it() {
    fixed(
        html,
        two(),
        "<!doctype html>\n<html lang=\"en\">\n  <head>\n    <meta charset=\"utf-8\" />\n    \
         <link rel=\"stylesheet\" href=\"a.css\">\n    <!-- a comment\n    with <div> -->\n    \
         <style>\n      body {\n        margin: 0;\n      }\n    </style>\n  </head>\n  <body>\n    \
         <img src=\"a.png\">\n    <br>\n    <ul>\n      <li>One</li>\n      <li>\n        Two\n      \
         </li>\n    </ul>\n    <div\n      class=\"a\"\n      id=\"b\"\n    >\n      text\n    </div>\n    \
         <script>\n      function f() {\n        return \"</div>\";\n      }\n    </script>\n  \
         </body>\n</html>\n",
    );
}

#[test]
fn pre_and_textarea_keep_their_lines() {
    let text = "<div>\n  <pre>\nkeep\n      this\n  </pre>\n  <p>x</p>\n</div>\n";
    let rope = Rope::from(text);
    let lines: Vec<_> = (0..rope.line_count()).map(Line::Row).collect();
    let answers = html(&rope, two(), &lines, &mut Budget::new());
    assert_eq!(answers[2], Answer::Keep);
    assert_eq!(answers[3], Answer::Keep);
    assert_eq!(answers[5], Answer::Column(2));
    assert_eq!(answers[6], Answer::Column(0));
}

#[test]
fn an_end_tag_closes_what_is_open_inside_it_and_a_stray_one_nothing() {
    // `<p>` left open is closed by its parent's end tag, as a browser closes it.
    fixed(
        html,
        two(),
        "<ul>\n  <li>\n    <p>a\n  </li>\n  <li>b</li>\n</ul>\n</span>\n<p>c</p>\n",
    );
}

#[test]
fn a_new_line_after_a_start_tag_goes_in() {
    let text = "<div>\n  <section>";
    assert_eq!(new_line(html, two(), text, text.len()), Answer::Column(4));
    let void = "<div>\n  <br>";
    assert_eq!(new_line(html, two(), void, void.len()), Answer::Column(2));
    let jsx = "const a = (\n  <div>";
    assert_eq!(
        new_line(javascript, two(), jsx, jsx.len()),
        Answer::Column(4)
    );
}

#[test]
fn a_typescript_comparison_opens_no_element() {
    fixed(
        typescript,
        two(),
        "function f<T>(a: Array<T>) {\n  if (a.length < 2) {\n    return;\n  }\n  return a;\n}\n",
    );
}

/// Past the lookback, a new line follows the line before: a level in after an opener, and
/// one out before a closer (ADR 0164, Q6).
#[test]
fn past_the_lookback_a_new_line_follows_the_line_before() {
    let mut text = String::from("{\n  \"a\": [\n");
    while text.len() < LINE_LOOKBACK + 4096 {
        text.push_str("    1,\n");
    }
    text.push_str("    {\n");
    let offset = text.len();
    text.push('}');
    let rope = Rope::from(text.as_str());
    let mut budget = Budget::new();
    let answers = json(&rope, two(), &[Line::New(offset)], &mut budget);
    // After `{` a level in, and the `}` the line starts with takes it back.
    assert_eq!(answers, [Answer::Column(4)]);
    let after_open = json(&rope, two(), &[Line::New(offset - 1)], &mut Budget::new());
    assert_eq!(after_open, [Answer::Column(6)]);
}

#[test]
fn a_script_ends_at_its_end_tag_wherever_it_comes() {
    // A browser ends a script at `</script`, even in one of its strings.
    fixed(
        html,
        two(),
        "<div>\n  <script>\n    const a = \"</script>\";\n  <p>x</p>\n</div>\n",
    );
    // Names compare ignoring case in HTML; `<textarea>` keeps its lines as `<pre>` does.
    fixed(html, two(), "<DIV>\n  <P>x</P>\n</div>\n");
    let text = "<form>\n  <textarea>\nkeep\n  </textarea>\n  <p>x</p>\n</form>\n";
    let rope = Rope::from(text);
    let lines: Vec<_> = (0..rope.line_count()).map(Line::Row).collect();
    let answers = html(&rope, two(), &lines, &mut Budget::new());
    assert_eq!(answers[2], Answer::Keep);
    assert_eq!(answers[4], Answer::Column(2));
}

#[test]
fn optional_chains_and_nullish_operands_go_on_with_their_statement() {
    fixed(
        typescript,
        two(),
        "const a = b\n  ?.c()\n  ?? d;\nconst e = f;\n",
    );
}

/// A TSX generic arrow's `<T,>` opens no element, and the lines after it are code.
#[test]
fn a_generic_s_parameters_open_no_element() {
    fixed(
        javascript,
        two(),
        "const f = <T,>(x: T) => {\n  return x;\n};\nconst g = <T extends U>(x: T) => x;\n\
         const h = 1;\n",
    );
}

/// Past the reach, a start tag that stays open sends a new line a level in, and an end tag
/// what follows it a level out (ADR 0164, Q6).
#[test]
fn past_the_reach_html_follows_its_tags() {
    let mut text = String::from("<html>\n  <body>\n");
    while text.len() < LINE_LOOKBACK + 4096 {
        text.push_str("    <p>x</p>\n");
    }
    text.push_str("    <div>\n");
    let offset = text.len();
    let rope = Rope::from(text.as_str());
    let answers = html(&rope, two(), &[Line::New(offset - 1)], &mut Budget::new());
    assert_eq!(answers, [Answer::Column(6)]);
    text.push_str("</div>");
    let rope = Rope::from(text.as_str());
    let answers = html(&rope, two(), &[Line::New(offset)], &mut Budget::new());
    assert_eq!(answers, [Answer::Column(4)]);
}

/// Crafted text costs a line's length, not its square: a regular expression that never
/// closes is looked for once a line, and an end tag of no open element searches nothing.
#[test]
fn crafted_lines_cost_their_length() {
    let regexes = format!("x = 1;\n{}\n", "/\\".repeat(30_000));
    let classes = format!("x = 1;\n{}\n", "/[".repeat(30_000));
    let tags = format!("{}{}\n", "<a>\n".repeat(256), "</>".repeat(20_000));
    let cases: [(Rules, &str); 3] = [
        (javascript, &regexes),
        (javascript, &classes),
        (html, &tags),
    ];
    for (rules, text) in cases {
        let rope = Rope::from(text);
        let started = std::time::Instant::now();
        let answers = rules(&rope, two(), &[Line::New(rope.len())], &mut Budget::new());
        let took = started.elapsed();
        assert_eq!(answers.len(), 1);
        // Linear, this takes well under a millisecond even in a debug build; the square of
        // 60 KB took seconds.
        assert!(took < std::time::Duration::from_millis(250), "{took:?}");
    }
}
