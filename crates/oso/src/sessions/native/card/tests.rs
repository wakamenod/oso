//! A native session's permission card (ADR 0165, step 2e-2): oso's rows around Claude's
//! fenced ones, the cut, and a question's answers; and what the wire reads of a call's input.
//!
//! Must not depend on: the app; the tests give what the call says.

use serde_json::json;

use super::{Card, FENCE, MAX_FENCED, MAX_ROWS, Questions, Step};
use crate::sessions::native::render::Face;
use crate::sessions::native::wire::card::{self, Asked, MAX_DIFF};

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
    let shown = card.shown();
    let rows: Vec<&str> = shown.text.lines().collect();
    assert_eq!(rows[0], "Bash wants to run");
    assert_eq!(rows[1], format!("{FENCE}$ ls [y] allow once  [n] deny"));
    assert_eq!(rows[2], format!("{FENCE}Why: look"));
    assert!(rows[3].starts_with("[y] allow once"), "the keys, last");
    assert_eq!(rows.len(), 4);
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
    assert_eq!(face_at(3), Some(Face::Prompt));
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
    let shown = card.shown();
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
    let shown = Card::Questions(questions.clone()).shown().text;
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
