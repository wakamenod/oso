//! The rules of the languages of brackets and tags (ADR 0164) in 2 MiB files: `RET` at one
//! cursor and at 1000, every line as `indent-region` asks, files with no anchor past their
//! first line, and crafted nesting.
//!
//! Must not depend on: crate internals; it uses only the public API, and the timing helpers
//! of `text_perf`.

use std::time::{Duration, Instant};

use oso_core::indent::{self, Answer, Budget, LINE_COST, LINE_LOOKBACK, Line, Settings, Style};
use oso_core::rope::Rope;

use super::super::{Lcg, MB, percentiles};

type Rules = fn(&Rope, Settings, &[Line], &mut Budget) -> Vec<Answer>;

/// A unit of each language as its formatter writes it, with anchors at column 0.
const CSS: &str = "/* Cards */\n.card > .title:hover,\n.card::after {\n  color: var(--accent);\n  background: url(\"a{.png\");\n}\n\n@media (min-width: 600px) {\n  .card {\n    padding: 0 calc(1rem + 2px);\n  }\n}\n\n";
const JSON_ITEM: &str = "    {\n      \"name\": \"item { [\",\n      \"values\": [1, 2, 3],\n      \"nested\": {\n        \"ok\": true\n      }\n    },\n";
const TSX: &str = "export function Card({ items }: Props) {\n  const total = items\n    .map((i) => i.value)\n    .reduce((a, b) => a + b, 0);\n  switch (total) {\n    case 0:\n      return null;\n    default:\n      break;\n  }\n  return (\n    <div className=\"card\">\n      {items.map((i) => (\n        <Item key={i.id} onClick={() => go(`/${i.id}`)} />\n      ))}\n      <p>Don't {total}</p>\n    </div>\n  );\n}\n\n";
const GO: &str = "func compute(items []Item) (int, error) {\n\ttotal := 0\n\tfor _, i := range items {\n\t\tswitch i.Kind {\n\t\tcase 1:\n\t\t\ttotal += i.Value\n\t\tdefault:\n\t\t\treturn 0, fmt.Errorf(`raw {\n%d`, i.Kind)\n\t\t}\n\t}\n\treturn total, nil\n}\n\n";
const HTML_ITEM: &str = "    <section class=\"card\">\n      <h2>Title &amp; more</h2>\n      <img src=\"a.png\">\n      <ul>\n        <li>One</li>\n        <li><a href=\"#\">Two</a></li>\n      </ul>\n      <!-- a comment -->\n    </section>\n";

fn two() -> Settings {
    Settings::new(2, Style::Spaces, 8).unwrap_or_default()
}

fn tabs() -> Settings {
    Settings::new(8, Style::Tabs, 8).unwrap_or_default()
}

/// One JSON document of about `bytes`: an array of objects, no anchor past its first line.
fn json_file(bytes: usize) -> String {
    format!(
        "{{\n  \"items\": [\n{}    {{}}\n  ]\n}}\n",
        JSON_ITEM.repeat(bytes / JSON_ITEM.len() + 1)
    )
}

/// One HTML page of about `bytes`: everything inside `<html>`, no anchor past its first lines.
fn html_file(bytes: usize) -> String {
    format!(
        "<!doctype html>\n<html>\n  <body>\n{}  </body>\n</html>\n",
        HTML_ITEM.repeat(bytes / HTML_ITEM.len() + 1)
    )
}

fn timed(
    rules: Rules,
    settings: Settings,
    rope: &Rope,
    lines: &[Line],
    budget: &mut Budget,
) -> Duration {
    let started = Instant::now();
    let _ = rules(rope, settings, lines, budget);
    started.elapsed()
}

/// `RET` at one cursor at random line ends, at 1000 spread cursors, and every line, in `text`.
fn costs(label: &str, rules: Rules, settings: Settings, text: &str) {
    let rope = Rope::from(text);
    let rows = rope.line_count();
    let mut rng = Lcg(23);
    let mut one: Vec<Duration> = (0..2000)
        .map(|_| {
            let at = rope.line_end(rng.below(rows));
            timed(rules, settings, &rope, &[Line::New(at)], &mut Budget::new())
        })
        .collect();
    let (p50, p99) = percentiles(&mut one);
    let most = one.iter().max().copied().unwrap_or_default();
    println!("{label:<14} RET at 1        p50 {p50:>9.2?}  p99 {p99:>9.2?}  max {most:>9.2?}");
    let lines: Vec<_> = (0..1000)
        .map(|index| Line::New(rope.line_end(index * rows / 1000)))
        .collect();
    let mut spent = false;
    let mut many: Vec<Duration> = (0..20)
        .map(|_| {
            let mut budget = Budget::new();
            let elapsed = timed(rules, settings, &rope, &lines, &mut budget);
            spent |= budget.is_spent();
            elapsed
        })
        .collect();
    let (p50, p99) = percentiles(&mut many);
    println!("{label:<14} RET at 1000     p50 {p50:>9.2?}  p99 {p99:>9.2?}  spent {spent}");
    let every: Vec<_> = (0..rows).map(Line::Row).collect();
    let needed = rope.len() + rows * LINE_COST + LINE_LOOKBACK;
    let mut region: Vec<Duration> = (0..5)
        .map(|_| {
            timed(
                rules,
                settings,
                &rope,
                &every,
                &mut Budget::with_limit(needed),
            )
        })
        .collect();
    let (p50, _) = percentiles(&mut region);
    println!("{label:<14} every line      p50 {p50:>9.2?}  ({rows} lines)");
}

/// Crafted nesting past the depth kept, and a line of tags, in 2 MiB.
fn crafted() {
    let cases: [(&str, Rules, String); 5] = [
        ("{ x 2 MiB", indent::css, "{\n".repeat(MB)),
        (
            "[ x 2 MiB",
            indent::json,
            "[".repeat(2 * MB / 8000).repeat(8000),
        ),
        ("<a> x 2 MiB", indent::html, "<a>\n".repeat(MB / 2)),
        ("${` x 2 MiB", indent::javascript, "`${\n".repeat(MB / 2)),
        ("<a {x}", indent::javascript, "x = <a b={(\n".repeat(MB / 6)),
    ];
    for (label, rules, text) in cases {
        let rope = Rope::from(text.as_str());
        let mut samples: Vec<Duration> = (0..200)
            .map(|index| {
                let at = rope.line_end(index * rope.line_count() / 200);
                timed(rules, two(), &rope, &[Line::New(at)], &mut Budget::new())
            })
            .collect();
        let (p50, p99) = percentiles(&mut samples);
        println!("crafted {label:<12} RET p50 {p50:>9.2?}  p99 {p99:>9.2?}");
    }
}

pub(super) fn code_costs() {
    println!("== Languages of brackets and tags (ADR 0164)");
    costs("CSS", indent::css, two(), &CSS.repeat(2 * MB / CSS.len()));
    costs("JSON", indent::json, two(), &json_file(2 * MB));
    costs(
        "TSX",
        indent::javascript,
        two(),
        &TSX.repeat(2 * MB / TSX.len()),
    );
    costs("Go", indent::go, tabs(), &GO.repeat(2 * MB / GO.len()));
    costs("HTML", indent::html, two(), &html_file(2 * MB));
    crafted();
}
