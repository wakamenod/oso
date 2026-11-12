//! Tests of the indentation rules (ADR 0115): the setting, what counts as indentation, the
//! TOML, Markdown and Fundamental rules, the shared pass and its budgets, and random text.
//!
//! Must not depend on: `buffer` or `editor`; they run on ropes alone.

use proptest::prelude::*;

use super::pass::{MAX_COLUMN, fallback, measure};
use super::{
    Answer, Budget, LINE_LOOKBACK, Line, Settings, Style, fundamental, markdown, markdown_stops,
    rust, toml,
};
use crate::display_map::TabWidth;
use crate::rope::Rope;

/// The rules of one mode.
pub(super) type Rules = fn(&Rope, Settings, &[Line], &mut Budget) -> Vec<Answer>;

/// The answer for every line of `text`, asked in one pass: a column, or none to keep.
pub(super) fn reindented(rules: Rules, text: &str) -> Vec<Option<usize>> {
    let rope = Rope::from(text);
    let lines: Vec<_> = (0..rope.line_count()).map(Line::Row).collect();
    let answers = rules(&rope, Settings::default(), &lines, &mut Budget::new());
    answers.into_iter().map(column).collect()
}

fn column(answer: Answer) -> Option<usize> {
    match answer {
        Answer::Column(column) => Some(column),
        Answer::Keep => None,
    }
}

/// The non-blank lines of `text` whose answer is a column other than the one they have:
/// their row, the answer and the column as written.
pub(super) fn misindented(rules: Rules, text: &str) -> Vec<(usize, usize, usize)> {
    let answers = reindented(rules, text);
    let lines = text.split('\n');
    lines
        .zip(answers)
        .enumerate()
        .filter_map(|(row, (line, answer))| {
            let written = measure(line, TabWidth::default());
            let answer = answer?;
            (!written.blank && answer != written.column).then_some((row, answer, written.column))
        })
        .collect()
}

/// The answers for new lines at `offsets` of `text`, in one pass.
pub(super) fn new_lines(rules: Rules, text: &str, offsets: &[usize]) -> Vec<Answer> {
    let rope = Rope::from(text);
    let lines: Vec<_> = offsets.iter().map(|&offset| Line::New(offset)).collect();
    rules(&rope, Settings::default(), &lines, &mut Budget::new())
}

#[test]
fn settings_take_one_to_thirty_two_and_write_indentation_in_their_style() {
    assert_eq!(Settings::new(0, Style::Spaces, 8), None);
    assert_eq!(Settings::new(4, Style::Spaces, 33), None);
    assert_eq!(Settings::new(33, Style::Spaces, 8), None);
    let settings = Settings::new(32, Style::Tabs, 1).unwrap();
    assert_eq!(
        (settings.indent_size(), settings.tab_width().get()),
        (32, 1)
    );
    let default = Settings::default();
    assert_eq!(
        (
            default.indent_size(),
            default.style(),
            default.tab_width().get()
        ),
        (4, Style::Spaces, 8)
    );
    assert_eq!(default.indentation(6), "      ");
    let tabs = Settings::new(4, Style::Tabs, 4).unwrap();
    assert_eq!(tabs.indentation(10), "\t\t  ");
    assert_eq!(tabs.indentation(3), "   ");
    assert_eq!(tabs.indentation(0), "");
}

#[test]
fn indentation_is_spaces_and_tabs_and_never_a_full_width_space() {
    let width = TabWidth::new(8);
    assert_eq!(measure("  \tx", width).column, 8);
    assert_eq!(measure(" \t \tx", width).column, 16);
    let wide = measure("\u{3000}text", width);
    assert_eq!((wide.bytes, wide.column, wide.blank), (0, 0, false));
    assert!(measure("   \r", width).blank);
    // Fundamental never copies a full-width space.
    assert_eq!(
        new_lines(fundamental, "\u{3000}日本語", &[12]),
        [Answer::Column(0)]
    );
}

/// The answer for each line of `text` asked alone, so that none follows another's answer.
fn alone(rules: Rules, text: &str) -> Vec<Option<usize>> {
    let rope = Rope::from(text);
    (0..rope.line_count())
        .map(|row| {
            let answers = rules(
                &rope,
                Settings::default(),
                &[Line::Row(row)],
                &mut Budget::new(),
            );
            column(answers[0])
        })
        .collect()
}

#[test]
fn fundamental_copies_the_line_before_unless_it_is_blank() {
    assert_eq!(
        alone(fundamental, "  a\nb\n    c\n\nd"),
        [Some(0), Some(2), Some(0), Some(4), Some(0)]
    );
    // Asked together, each line follows the one before as the command leaves it.
    assert_eq!(reindented(fundamental, "  a\nb\n    c\n\nd"), [Some(0); 5]);
    assert_eq!(
        new_lines(fundamental, "    abc\n  \n", &[7, 10, 11]),
        [Answer::Column(4), Answer::Column(0), Answer::Column(0)]
    );
    // A tab counts to the next stop.
    assert_eq!(new_lines(fundamental, "\t x", &[3]), [Answer::Column(9)]);
}

#[test]
fn toml_indents_inside_brackets_and_follows_the_lines_outside() {
    let text = "[a]\nx = [\n    1,\n    { b = 2 },\n]\ny = 3\n";
    assert!(
        misindented(toml, text).is_empty(),
        "{:?}",
        misindented(toml, text)
    );
    let text = "[a]\n  x = 1\n  y = [\n    1,\n  ]\n  z = 2\n";
    assert_eq!(
        alone(toml, text),
        [
            Some(0),
            Some(0),
            Some(2),
            Some(6),
            Some(2),
            Some(2),
            Some(2)
        ]
    );
    // Strings hold no brackets, and a multi-line string keeps its lines.
    let text = "a = \"[\"\nb = '''\n  [x\n'''\nc = \"\"\"\n\\\"\"\"\n\"\"\"\nd = 1 # [\ne = 2\n";
    assert_eq!(
        reindented(toml, text),
        [
            Some(0),
            Some(0),
            None,
            None,
            Some(0),
            None,
            None,
            Some(0),
            Some(0),
            Some(0)
        ]
    );
    assert_eq!(new_lines(toml, "x = [", &[5]), [Answer::Column(4)]);
    assert_eq!(new_lines(toml, "x = []", &[5]), [Answer::Column(0)]);
}

#[test]
fn markdown_new_lines_go_to_the_items_text() {
    let cases: [(&str, usize, usize); 9] = [
        ("- item", 6, 2),
        ("  * item", 8, 4),
        ("+   item", 8, 4),
        ("12. item", 8, 4),
        ("1) item", 7, 3),
        ("-      code", 11, 2),
        ("    text", 8, 4),
        ("-not an item", 12, 0),
        ("```\n- in a fence", 16, 0),
    ];
    for (text, offset, expected) in cases {
        assert_eq!(
            new_lines(markdown, text, &[offset]),
            [Answer::Column(expected)],
            "{text:?}"
        );
    }
    // Existing lines keep: TAB cycles through the stops instead.
    assert_eq!(reindented(markdown, "- a\nb"), [None, None]);
}

#[test]
fn markdown_stops_are_the_enclosing_items_then_the_margin() {
    let text = "- a\n  - b\n    1. c\nx\n\nplain\n- d\n```\n- e\n";
    let rope = Rope::from(text);
    let stops = |row| markdown_stops(&rope, Settings::default(), row, &mut Budget::new());
    assert_eq!(stops(0), [0]);
    assert_eq!(stops(1), [2, 0]);
    assert_eq!(stops(3), [7, 4, 2, 0]);
    // A paragraph after a blank line ends the lists.
    assert_eq!(stops(6), [0]);
    // Inside a fence, only the margin.
    assert_eq!(stops(8), [0]);
}

#[test]
fn lines_out_of_order_or_twice_are_answered_without_a_second_pass() {
    let rope = Rope::from("fn f() {\n    x\n}\n");
    let lines = [Line::Row(1), Line::Row(1), Line::Row(0), Line::New(13)];
    let answers = rust(&rope, Settings::default(), &lines, &mut Budget::new());
    assert_eq!(
        answers,
        [
            Answer::Column(4),
            Answer::Column(4),
            Answer::Keep,
            Answer::Column(4)
        ]
    );
}

#[test]
fn a_line_after_one_out_of_order_is_found_by_its_row() {
    let text = "fn f() {\n    a(\n        b,\n    );\n    c\n}\n";
    let rope = Rope::from(text);
    let lines = [Line::Row(4), Line::Row(1), Line::Row(2)];
    let answers = rust(&rope, Settings::default(), &lines, &mut Budget::new());
    // The line after the one out of order starts a pass of its own.
    assert_eq!(
        answers,
        [Answer::Column(4), Answer::Keep, Answer::Column(8)]
    );
}

#[test]
fn past_the_budget_a_new_line_copies_and_an_existing_one_keeps() {
    let text = format!("fn f() {{\n    a(\n{}", "        b,\n".repeat(100));
    let rope = Rope::from(text.as_str());
    let mut budget = Budget::with_limit(64);
    let answers = rust(
        &rope,
        Settings::default(),
        &[Line::Row(50), Line::New(rope.len())],
        &mut budget,
    );
    assert!(budget.is_spent());
    assert_eq!(answers, [Answer::Keep, Answer::Column(8)]);
}

#[test]
fn an_anchor_past_the_reach_is_not_looked_for() {
    let body = "        x,\n".repeat(LINE_LOOKBACK / 11 + 10);
    let text = format!("fn f() {{\n    g(\n{body}    )\n}}\n");
    let rope = Rope::from(text.as_str());
    let last = rope.line_count() - 3;
    let mut budget = Budget::new();
    let answers = rust(
        &rope,
        Settings::default(),
        &[Line::Row(last), Line::Row(last + 1)],
        &mut budget,
    );
    assert_eq!(answers, [Answer::Keep, Answer::Keep]);
    // From the start, one pass carries the state to the end within the reach of each line.
    let lines: Vec<_> = (0..rope.line_count()).map(Line::Row).collect();
    let mut budget = Budget::with_limit(usize::MAX);
    let answers = rust(&rope, Settings::default(), &lines, &mut budget);
    assert_eq!(
        answers[last..=last + 1],
        [Answer::Column(4), Answer::Column(0)]
    );
}

#[test]
fn brackets_nested_deeper_than_the_lexer_keeps_fall_back() {
    for depth in [10_000, 100_000] {
        let text = format!(
            "fn f() {{\n{}\nx\n{}\n}}\n",
            "(".repeat(depth),
            ")".repeat(depth)
        );
        let rope = Rope::from(text.as_str());
        let lines: Vec<_> = (0..rope.line_count()).map(Line::Row).collect();
        let answers = rust(
            &rope,
            Settings::default(),
            &lines,
            &mut Budget::with_limit(usize::MAX),
        );
        // Past the reach, a line keeps its indentation too.
        assert_eq!(answers[2], Answer::Keep, "{depth}");
        let close = if depth < LINE_LOOKBACK {
            Answer::Column(0)
        } else {
            Answer::Keep
        };
        assert_eq!(answers[4], close, "{depth}");
        let text = format!("x = {}\ny\n", "[".repeat(depth));
        let rope = Rope::from(text.as_str());
        let answers = toml(
            &rope,
            Settings::default(),
            &[Line::Row(1)],
            &mut Budget::new(),
        );
        assert_eq!(answers, [Answer::Keep]);
    }
}

#[test]
fn a_long_line_is_read_within_the_budget() {
    let text = format!("fn f() {{ {}", "a(b), ".repeat(8_000));
    let rope = Rope::from(text.as_str());
    // After a `,`, so the line it leaves ends a statement.
    let middle = "fn f() { ".len() + "a(b), ".len() * 4_000;
    let mut budget = Budget::new();
    let answers = rust(
        &rope,
        Settings::default(),
        &[Line::New(middle)],
        &mut budget,
    );
    assert_eq!(answers, [Answer::Column(4)]);
    let mut budget = Budget::with_limit(1000);
    let answers = rust(
        &rope,
        Settings::default(),
        &[Line::New(middle)],
        &mut budget,
    );
    assert!(budget.is_spent());
    assert_eq!(answers, [Answer::Column(0)]);
}

#[test]
fn an_anchor_looked_for_after_wide_characters_starts_its_line() {
    let cases = [
        ("あ".repeat(100) + "\nあいう\nx", 310),
        ("д".repeat(200) + "\nдд\nx", 405),
        ("𝄞".repeat(80) + "\n}\n[日本\nx", 330),
    ];
    for (text, offset) in cases {
        let rope = Rope::from(text.as_str());
        let lines = [
            Line::New(offset.min(rope.len())),
            Line::Row(rope.line_count() - 1),
        ];
        for rules in ALL_RULES {
            let answers = rules(&rope, Settings::default(), &lines, &mut Budget::new());
            assert_eq!(answers.len(), 2, "{text:?}");
        }
        let answers = fundamental(&rope, Settings::default(), &lines[..1], &mut Budget::new());
        assert_eq!(answers, [Answer::Column(0)], "{text:?}");
    }
}

#[test]
fn lines_that_go_on_with_a_statement_do_not_stack() {
    let text = "fn f() {\n".to_owned() + &"x\n".repeat(1000);
    let most = reindented(rust, &text).into_iter().flatten().max();
    assert_eq!(most, Some(8));
}

#[test]
fn no_column_past_the_deepest_is_answered() {
    let deep = " ".repeat(MAX_COLUMN);
    let text = format!("fn f() {{\n{deep}g(\n");
    let rope = Rope::from(text.as_str());
    let answers = rust(
        &rope,
        Settings::default(),
        &[Line::Row(2), Line::New(rope.len())],
        &mut Budget::new(),
    );
    assert_eq!(answers, [Answer::Keep, Answer::Keep]);
    let text = format!("{deep} x\n");
    let rope = Rope::from(text.as_str());
    let answers = fundamental(
        &rope,
        Settings::default(),
        &[Line::Row(1)],
        &mut Budget::new(),
    );
    assert_eq!(answers, [Answer::Keep]);
}

#[test]
fn a_line_of_block_comments_is_measured_once() {
    // An opener every few bytes, after a tab and a wide character: the column of the one
    // left open is its cells from the line's start.
    let text = format!(
        "fn f() {{\n    x\t日{}/* open\n * star\n",
        "/**/".repeat(900)
    );
    let answers = reindented(rust, &text);
    assert_eq!(answers[2], Some(4 + 4 + 2 + 4 * 900 + 1));
}

/// Pieces of text random tests are built from: what each lexer reacts to, and the atoms of
/// the rope's own tests.
const PIECES: &[&str] = &[
    "{", "}", "(", ")", "[", "]", "\"", "'", "r#\"", "\"#", "/*", "*/", "//", "#", "#[", ";", ",",
    "\\", "fn ", "where ", "b'", "'a", "\"\"\"", "'''", "- ", "1. ", "```", "~~~", "x", " ",
    "    ", "\t", "\n", "\r\n", "\r", "日本", "\u{3000}", "é", "👍", "\0",
];

fn text_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PIECES), 0..200).prop_map(|pieces| pieces.concat())
}

fn lines_strategy() -> impl Strategy<Value = Vec<(bool, usize)>> {
    prop::collection::vec((any::<bool>(), 0..400usize), 0..12)
}

/// Where `line` starts.
fn start(rope: &Rope, line: Line) -> usize {
    match line {
        Line::Row(row) => rope.line_start(row),
        Line::New(offset) => rope.clip_offset(offset, crate::sum_tree::Bias::Left),
    }
}

const ALL_RULES: [Rules; 4] = [rust, toml, markdown, fundamental];

proptest! {
    #[test]
    fn long_text_in_one_script_never_panics(
        script in prop::sample::select(&["é", "あ", "д", "𝄞", "a"][..]),
        lines in prop::collection::vec((0..40usize, prop::sample::select(&["", "}", "[", "fn x", "- "][..])), 1..80),
        asked in prop::collection::vec(0..20_000usize, 1..20),
    ) {
        let text: String = lines
            .iter()
            .map(|(count, head)| format!("{head}{}\n", script.repeat(count * 7)))
            .collect();
        let rope = Rope::from(text.as_str());
        let mut asked: Vec<_> = asked.into_iter().map(|at| rope.clip_offset(at, crate::sum_tree::Bias::Left)).collect();
        asked.sort_unstable();
        let lines: Vec<_> = asked.iter().map(|&at| Line::New(at)).collect();
        let rows: Vec<_> = (0..rope.line_count()).map(Line::Row).collect();
        for rules in ALL_RULES {
            prop_assert_eq!(rules(&rope, Settings::default(), &lines, &mut Budget::new()).len(), lines.len());
            prop_assert_eq!(rules(&rope, Settings::default(), &rows, &mut Budget::new()).len(), rows.len());
        }
    }

    #[test]
    fn random_text_never_panics_and_answers_every_line(
        text in text_strategy(),
        asked in lines_strategy(),
        sorted in any::<bool>(),
        bytes in prop::option::of(0..2000usize),
    ) {
        let rope = Rope::from(text.as_str());
        let mut lines: Vec<_> = asked
            .into_iter()
            .map(|(new, at)| if new { Line::New(at) } else { Line::Row(at % 40) })
            .collect();
        if sorted {
            lines.sort_by_key(|&line| start(&rope, line));
        }
        for rules in ALL_RULES {
            let mut budget = bytes.map_or_else(Budget::new, Budget::with_limit);
            let answers = rules(&rope, Settings::default(), &lines, &mut budget);
            prop_assert_eq!(answers.len(), lines.len());
            // A smaller budget only turns the answers from the first line it runs out on
            // into fallbacks.
            let mut unbounded = Budget::with_limit(usize::MAX);
            let full = rules(&rope, Settings::default(), &lines, &mut unbounded);
            let same = answers.iter().zip(&full).take_while(|(a, b)| a == b).count();
            for index in same..lines.len() {
                // A line asked again where the one before starts has that one's answer.
                let again = index > 0 && start(&rope, lines[index]) == start(&rope, lines[index - 1]);
                if !again {
                    prop_assert!(budget.is_spent());
                    prop_assert_eq!(answers[index], fallback(&rope, Settings::default(), lines[index], &mut Budget::new()));
                }
            }
        }
        for row in 0..rope.line_count().min(8) {
            let stops = markdown_stops(&rope, Settings::default(), row, &mut Budget::new());
            prop_assert_eq!(stops.last(), Some(&0));
        }
    }

}
