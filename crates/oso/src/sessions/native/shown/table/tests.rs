//! A reply's tables as drawn: their box at each width, what is wrapped and what stays as
//! written, and that no line of one is wider than it measures.
//!
//! Must not depend on: rows, the syntax threads or the theme.

use oso_core::display_map::text_clusters;
use oso_core::minibuffer::cells_of;
use proptest::prelude::*;

use super::super::place::MAX_TABLES;
use super::super::{LineStart, MAX_MARKED, MAX_TEXT, Style, again, mark, walk};
use super::grid::{apart, letter};
use super::{BYTES_A_LINE, Budget, FEW_LINES, MAX_COLUMNS};

/// `text` as drawn in `width` cells: each line, and whether it is a table's.
fn lines(text: &str, width: usize, streams: bool) -> Vec<(String, bool)> {
    let mut lines = Vec::new();
    walk(
        text,
        LineStart::default(),
        streams,
        width,
        |pieces, table| {
            let line = pieces.iter().map(|piece| piece.text).collect();
            lines.push((line, table.is_some()));
        },
    );
    lines
}

fn shown(text: &str, width: usize) -> String {
    let lines = lines(text, width, false);
    lines.iter().map(|(line, _)| format!("{line}\n")).collect()
}

/// Each of `cases` is drawn as written.
fn stays(cases: &[&str]) {
    for case in cases {
        assert_eq!(shown(case, 98), format!("{case}\n"));
    }
}

#[test]
fn a_table_is_drawn_in_a_box() {
    let text = "下の表:\n| 入力 | 以前 | いま |\n|---|:-:|--:|\n| `/settings` | `/settings` | a |\n| https://evil.example | 外部へ移動 |\n\nx";
    assert_eq!(
        shown(text, 98),
        "\
下の表:
┌──────────────────────┬────────────┬──────┐
│ 入力                 │    以前    │ いま │
├──────────────────────┼────────────┼──────┤
│ /settings            │ /settings  │    a │
│ https://evil.example │ 外部へ移動 │      │
└──────────────────────┴────────────┴──────┘

x
"
    );
    // A header alone, one column, in a list's item, with no pipe at its end, and the reply's end.
    assert_eq!(
        shown("- a\n  | b |\n  |---\n\n| c", 98),
        "• a\n  ┌───┐\n  │ b │\n  └───┘\n\n| c\n"
    );
}

#[test]
fn a_table_s_rules_and_header_have_faces_of_their_own() {
    let mut faces = Vec::new();
    let text = "| a `b` |\n|---|\n| **c** |";
    walk(text, LineStart::default(), false, 98, |pieces, _| {
        for piece in pieces {
            faces.push((piece.text.to_owned(), piece.style));
            assert_eq!(piece.from, None, "no parse's highlights lie on it");
        }
    });
    let rule = |text: &str| (text.to_owned(), Some(Style::Rule));
    assert_eq!(
        faces,
        [
            rule("┌─────┐"),
            rule("│ "),
            ("a ".to_owned(), Some(Style::Strong)),
            ("b".to_owned(), Some(Style::Code)),
            rule(" │"),
            rule("├─────┤"),
            rule("│ "),
            ("c".to_owned(), Some(Style::Strong)),
            rule("   │"),
            rule("└─────┘"),
        ]
    );
}

#[test]
fn wide_cells_wrap_and_rows_are_then_ruled_apart() {
    let text = "| Key | What it does |\n|---|---|\n| w | copies the reply's raw Markdown to the kill ring |\n| RET | 現在の行にあるツール呼び出しの結果を、すべて開きます。 |";
    assert_eq!(
        shown(text, 34),
        "\
┌─────┬──────────────────────────┐
│ Key │ What it does             │
├─────┼──────────────────────────┤
│ w   │ copies the reply's raw   │
│     │ Markdown to the kill     │
│     │ ring                     │
├─────┼──────────────────────────┤
│ RET │ 現在の行にあるツール呼び │
│     │ 出しの結果を、すべて開き │
│     │ ます。                   │
└─────┴──────────────────────────┘
"
    );
    // A word longer than its column is broken in it, and a line starts with no stop.
    assert_eq!(
        shown("| a |\n|---|\n| 12345678901 |\n| あいう。え |", 10),
        "\
┌────────┐
│ a      │
├────────┤
│ 123456↩│
│ 78901  │
├────────┤
│ あい   │
│ う。え │
└────────┘
"
    );
}

#[test]
fn columns_give_way_the_widest_first_and_a_table_too_wide_is_as_written() {
    let long = "word ".repeat(40);
    let text = format!("| id | {long} | {long} |\n|---|---|---|\n| 1 | x | y |");
    for width in [98, 60, 40, 30] {
        let drawn = lines(&text, width, false);
        assert!(
            drawn.iter().all(|(line, _)| cells_of(line) == width),
            "{drawn:?}"
        );
        assert!(
            drawn[3]
                .0
                .trim_start_matches(['│', ' '])
                .starts_with("word"),
            "{drawn:?}"
        );
        // The narrow column keeps its width.
        assert!(drawn[0].0.starts_with("┌────┬"), "{drawn:?}");
    }
    // Nine columns of four cells each need 64.
    let cells = "| abcdefgh ".repeat(9) + "|";
    let text = format!("{cells}\n{}|\n{cells}", "|---".repeat(9));
    assert!(shown(&text, 64).starts_with('┌'));
    assert_eq!(shown(&text, 63), format!("{text}\n"));
    assert_eq!(shown("| a |\n|---|", 0), "| a |\n|---|\n");
}

#[test]
fn what_is_no_table_stays() {
    stays(&[
        "| a | b |",
        "| a | b |\n| c | d |",
        "| a | b |\n|---|",
        "| a |\n|---|---|",
        "| a | b |\n|-x-|---|",
        "| a | b |\n||--|",
        "a | b\n---|---",
        "| t |\n| a |\n|---|",
        "    | a |\n    |---|",
        "\t| a |\n\t|---|",
    ]);
    // Inside a fence it is code, and a quote's keeps its bars.
    assert_eq!(shown("```\n| a |\n|---|\n```", 98), "| a |\n|---|\n");
    assert_eq!(shown("> | a |\n> |---|", 98), "│ | a |\n│ |---|\n");
    // A line with no pipe at its start ends it.
    assert_eq!(
        shown("| a |\n|---|\n| b |\n**c** | d", 98),
        "┌───┐\n│ a │\n├───┤\n│ b │\n└───┘\nc | d\n"
    );
    // After a fence, a blank line sets it apart.
    assert_eq!(
        shown("```\na\n```\n| b |\n|---|", 98),
        "a\n\n┌───┐\n│ b │\n└───┘\n"
    );
}

#[test]
fn no_word_of_a_row_goes() {
    // Cells past the last column are the last's, and a pipe's backslash goes, no other.
    assert_eq!(
        shown(
            "| a | b |\n|---|---|\n| `x \\| y` | p | q | r |\n| \\* \\\\ | \\a |",
            98
        ),
        "\
┌───────┬───────────┐
│ a     │ b         │
├───────┼───────────┤
│ x | y │ p | q | r │
│ \\* \\\\ │ \\a        │
└───────┴───────────┘
"
    );
}

#[test]
fn a_link_in_a_cell_hides_its_target_only_beside_plain_words() {
    let row = |cells: &str| {
        let drawn = shown(&format!("| a | b |\n|---|---|\n{cells}"), 98);
        drawn.lines().nth(3).map(str::to_owned).unwrap_or_default()
    };
    assert_eq!(
        row("| [docs](https://a.b/) | all of it |"),
        "│ docs │ all of it │"
    );
    for other in [
        "owasp.org",
        "https://owasp.org/",
        "(x/y)",
        "[c](https://d.e/)",
    ] {
        let drawn = row(&format!("| [docs](https://a.b/) | {other} |"));
        assert!(drawn.contains("docs (https://a.b/)"), "{drawn}");
    }
}

#[test]
fn a_cell_shows_as_it_measures_or_its_table_stays() {
    // Escapes are measured as they show: a control, a bidi control, an invisible character.
    let text = "| a | b |\n|---|---|\n| \u{1b}[31m | x |\n| \u{202e}y\u{200b} | z |\n| 👩\u{200d}💻 ⚠\u{fe0f} é e\u{301} カ\u{3099} | w |";
    let drawn = lines(text, 98, false);
    let width = cells_of(&drawn[0].0);
    assert!(
        drawn.iter().all(|(line, _)| cells_of(line) == width),
        "{drawn:?}"
    );
    assert!(drawn[0].0.starts_with('┌'));
    // A tab is a space, as its width is its place's.
    assert!(shown("| a\tb |\n|---|", 98).contains("│ a b │"));
    // What would join the space beside it, or the display cuts.
    let marks = "\u{301}".repeat(40);
    for cell in [
        "\u{301}a",
        "\u{200d}a",
        "\u{600}",
        "a\u{fe0f}\u{20e3}\u{301} \u{301}",
        &marks,
    ] {
        let text = format!("| a |\n|---|\n| {cell} |");
        let drawn = lines(&text, 98, false);
        let boxed = drawn[0].0.starts_with('┌');
        assert!(
            !boxed
                || drawn
                    .iter()
                    .all(|(line, _)| cells_of(line) == cells_of(&drawn[0].0)),
            "{cell:?}: {drawn:?}"
        );
    }
    stays(&["| a |\n|---|\n| \u{301}a |", "| a |\n|---|\n| a \u{600} |"]);
    // A mark between two characters that could join stays.
    assert!(shown("| a |\n|---|\n| 字*\u{180b}a* |", 98).contains("字*\u{180b}a*"));
}

#[test]
fn an_address_broken_in_a_cell_says_so() {
    let word = "abcdefghijklmnop";
    let text = format!(
        "|{0}|{0}|{0}|{0}| a |\n|-|-|-|-|-|\n| | | | | https://github.comexample.dev/a b |",
        word
    );
    let drawn = shown(&text, 98);
    assert!(drawn.contains("│ https://github.com↩│\n"), "{drawn}");
    assert!(drawn.contains("│ example.dev/a b    │\n"), "{drawn}");
    // Beside a wide character too, where no other text is marked.
    let drawn = shown("| a |\n|-|\n| https://a.b字/c |\n| ab字字字 |", 14);
    assert!(drawn.contains("│ https://a.↩│\n│ b字/c      │"), "{drawn}");
    assert!(shown("| a |\n|-|\n| 字字字字 |", 10).contains("│ 字字字 │\n│ 字     │"));
}

#[test]
fn what_is_past_a_limit_is_as_written() {
    let wide = "| a ".repeat(MAX_COLUMNS + 1) + "|\n" + &"|-".repeat(MAX_COLUMNS + 1) + "|";
    let long = format!("| a |\n|---|\n| {} |", "b".repeat(MAX_MARKED));
    let huge = format!("| a |\n|---|\n{}", "| some text |\n".repeat(4700));
    // Few bytes a line, and many lines.
    let thin = format!("| a |\n|---|\n{}", "|\n".repeat(FEW_LINES * 2));
    let tall = format!("| a |\n|---|\n{}", "| b |\n".repeat(2045));
    for text in [wide, long, huge, thin, tall] {
        assert_eq!(shown(&text, 200).trim_end(), text.trim_end());
    }
    let fits = format!("| a |\n|---|\n{}", "| b |\n".repeat(2044));
    assert_eq!(lines(&fits, 98, false).len(), 2048);
    // The limits are the reply's: what they leave no room for is as written, as is what is past
    // those of its marks and its tables.
    let text = format!("{fits}\n| b |\n|---|\n");
    assert!(shown(&text, 98).ends_with("\n| b |\n|---|\n"));
    let text = "a\n".repeat(MAX_TEXT / 2) + "| b |\n|---|\n";
    assert!(shown(&text, 98).ends_with("\n| b |\n|---|\n"));
    let text = "| b |\n|---|\n\n".repeat(usize::from(MAX_TABLES) + 1);
    assert_eq!(
        shown(&text, 98).matches('┌').count(),
        usize::from(MAX_TABLES)
    );
}

/// Tables that are drawn, and that fail only once they were laid out.
fn crafted() -> Vec<String> {
    let columns = MAX_COLUMNS - 1;
    let empty = "|".repeat(columns - 1);
    let dashes = "|-".repeat(columns);
    let late = format!(
        "|{}\n|-\n| {} |\n{}\n",
        "a",
        "x".repeat(100),
        "|\n".repeat(1030)
    );
    let rows = "|a".repeat(columns) + "\n";
    vec![
        format!(
            "{empty}| {} |\n{dashes}\n{}\n",
            "x".repeat(30),
            "|\n".repeat(1030)
        ),
        late,
        format!("{rows}{dashes}\n{}| \u{301}a |\n\n", rows.repeat(600)),
        format!("{rows}{dashes}\n{}\n", rows.repeat(600)),
        format!(
            "| a | b | c |\n|-|-|-|\n{}\n",
            format!("| {} |\n", "x ".repeat(4000)).repeat(8)
        ),
        format!("| a |\n|-|\n{}\n", "|\u{200b}\n".repeat(1900)),
        format!(
            "| a |\n|-|\n{}\n",
            format!("| {} |\n", "\u{7f}".repeat(16_000)).repeat(3)
        ),
        "|a\n|-\n\n".to_owned(),
    ]
}

#[test]
fn a_crafted_reply_s_tables_draw_and_cost_a_bounded_amount() {
    let whole = Budget::default().taken();
    let most = Budget {
        bytes: 0,
        cells: 0,
        lines: 0,
    };
    assert_eq!(whole, 0);
    for unit in crafted() {
        let text = unit.repeat((256 << 10) / unit.len() + 1);
        for width in [20, 60, 98, 200] {
            let end = mark(&text, LineStart::default(), false, width);
            assert!(end.table_cells() <= most.taken());
            let drawn = shown(&text, width);
            assert!(
                drawn.len() <= text.len() + 2048 * 3 * (width + 1),
                "{width}"
            );
            // Few bytes draw no more than some lines each.
            let lines = drawn.lines().count();
            let written = text.lines().count();
            assert!(lines <= written + 3 * usize::from(MAX_TABLES) + text.len() / BYTES_A_LINE);
        }
    }
    // One that fails late is taken from what the others may.
    let late = crafted().swap_remove(1);
    let end = mark(&late, LineStart::default(), false, 98);
    assert!(end.table_cells() > 8 * FEW_LINES, "{}", end.table_cells());
}

#[test]
fn letters_taken_to_stand_apart_do() {
    for c in ('\u{590}'..='\u{e7f}').filter(|c| letter(*c)) {
        // With a letter that is none, beside it, so that it is looked at.
        let mut between = String::new();
        let cluster = c.to_string();
        assert!(apart(&cluster, &mut between));
        assert!(between.is_empty(), "not looked at");
        let line = format!(" {cluster} ");
        let found: Vec<_> = text_clusters(&line, 0)
            .map(|cluster| cluster.text)
            .collect();
        assert_eq!(found, [" ", cluster.as_str(), " "]);
    }
}

#[test]
fn a_table_that_streams_is_as_written_until_it_ends() {
    let text = "a\n| b | c |\n|---|---|\n| 1 | 2 |\n";
    let drawn = lines(text, 98, true);
    let raw: String = drawn.iter().map(|(line, _)| format!("{line}\n")).collect();
    assert_eq!(raw, text);
    assert!(drawn.iter().all(|(_, table)| !table), "none that is whole");
    let mark = walk(text, LineStart::default(), true, 98, |_, _| {});
    assert_eq!(
        (mark.from, mark.lines),
        (text.len(), 4),
        "its lines are not read again"
    );
    // More of it, and its next line, not yet whole.
    for more in ["| 3", "| 3 | 4 |\n", "x"] {
        assert_eq!(again(&format!("{text}{more}"), mark, true), mark);
    }
    // Its end, by a line that is none of its or the reply's: from its header.
    for (more, streams) in [("\n", true), ("x\n", true), ("", false), ("| 3", false)] {
        let text = format!("{text}{more}");
        let at = again(&text, mark, streams);
        assert_eq!((at.from, at.lines), (2, 1));
        let mut first = String::new();
        walk(&text, at, streams, 98, |pieces, table| {
            if first.is_empty() && table.is_some() {
                first = pieces.iter().map(|piece| piece.text).collect();
            }
        });
        assert_eq!(first, "┌───┬───┐");
    }
}

proptest! {
    /// Whatever is in its cells, every line of a table in a box is as wide as its rules, within
    /// the width given, its rules in its own face, and its text in order; and its lines say
    /// whose they are, whatever the width.
    #[test]
    fn every_line_of_a_box_is_as_wide_as_it(
        rows in prop::collection::vec(
            prop::collection::vec(
                prop_oneof![
                    "[a-z0-9 .`*|\\\\]{0,12}",
                    "[字あーア、。「」（ａｱ ]{0,12}",
                    "(👩\u{200d}💻|⚠\u{fe0f}|🇯🇵|e\u{301}|カ\u{3099}|\u{202e}|\u{200b}|\u{7}|\t|\u{600}|\u{301}|א|ن|ก็| ){0,6}",
                    "[ -~]{0,200}",
                    any::<String>(),
                ],
                0..5,
            ),
            1..6,
        ),
        columns in 1usize..5,
        width in 0usize..110,
    ) {
        let mut text = "| h ".repeat(columns) + "|\n" + &"|-".repeat(columns) + "|\n";
        for row in &rows {
            for cell in row {
                text.push_str("| ");
                text.extend(cell.chars().filter(|c| *c != '\n'));
            }
            text.push_str("|\n");
        }
        let drawn = lines(&text, width, false);
        prop_assert!(drawn.iter().all(|(_, table)| *table));
        if !drawn[0].0.starts_with('┌') {
            let raw: String = drawn.iter().map(|(line, _)| format!("{line}\n")).collect();
            prop_assert_eq!(raw, text.replace("\r\n", "\n"));
            return Ok(());
        }
        let across = cells_of(&drawn[0].0);
        prop_assert!(across <= width);
        for (line, _) in &drawn {
            // As the buffer draws it: no tab is left, and nothing joins a rule.
            prop_assert_eq!(cells_of(line), across, "{:?}", line);
            prop_assert!(line.starts_with(['┌', '│', '├', '└']));
            prop_assert!(line.ends_with(['┐', '│', '┤', '┘']));
        }
        let letters = |text: &str| text.chars().filter(|c| c.is_alphabetic() || *c == '\u{202e}').collect::<Vec<_>>();
        let mut written = letters(&text.replace("](", ""));
        let mut shown = letters(&drawn.iter().map(|(line, _)| line.as_str()).collect::<String>());
        // A cell that wraps is read by lines: the same letters, each as often.
        written.sort_unstable();
        shown.sort_unstable();
        prop_assert_eq!(shown, written);
    }
}
