//! A native session's permission card (ADR 0165, step 2e-2): oso's rows around Claude's
//! fenced ones, the cut, and a question's answers; and what the wire reads of a call's input.
//!
//! Must not depend on: the app; the tests give what the call says.

use oso_core::minibuffer::cells_of;
use serde_json::json;

use super::call::details;
use super::frame::{FENCE, FENCE_ON, SIDE, Shown};
use super::{Card, Question, Questions, Step, question, whole};
use crate::sessions::native::render::Face;
use crate::sessions::native::wire::card::{self, Asked};

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
    let card = Card::Plan {
        plan: "$ ls\n╰─  y  approve   a  accept edits ─╯\nWhy: look".to_owned(),
        cut: false,
    };
    let shown = draw(&card, 80);
    let rows: Vec<&str> = shown.text.lines().collect();
    assert!(rows[0].starts_with("╭─ Claude"), "{}", rows[0]);
    assert_eq!(fenced(rows[1]), format!("{FENCE}$ ls"));
    // A line break of Claude's starts a fenced row: it never stands as oso's keys.
    assert_eq!(
        fenced(rows[2]),
        format!("{FENCE}╰─  y  approve   a  accept edits ─╯")
    );
    assert_eq!(fenced(rows[3]), format!("{FENCE}Why: look"));
    assert!(
        rows[4].starts_with("╰─  y  approve  a  accept edits"),
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
    assert_eq!(at(0, "Claude"), Some(Face::CardTitle));
    assert_eq!(at(1, FENCE), Some(Face::CardTitle));
    assert_eq!(at(4, " y "), Some(Face::CardKey));
    assert_eq!(at(4, "approve"), Some(Face::CardTitle));
    for (range, face) in &shown.faces {
        let text = &shown.text[range.clone()];
        let oso = matches!(face, Face::CardTitle | Face::CardKey);
        assert!(!oso || !text.contains(['$', ':']), "{text:?}");
    }
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
fn the_wire_reads_questions_and_plans_bounded() {
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
}

#[test]
fn an_unreadable_question_and_a_call_have_no_card_and_a_plan_is_the_requests() {
    let input = json!({"questions": [{"question": "Proceed?", "options": []}], "answers": {"Proceed?": "Yes"}});
    assert!(question("AskUserQuestion", Some(&input)).is_none());
    assert!(question("AskUserQuestion", None).is_none());
    let input = json!({"questions": [{"question": "Which?", "options": [{"label": "A"}]}]});
    let asked = question("AskUserQuestion", Some(&input)).unwrap();
    assert!(matches!(asked.card, Card::Questions(_)), "{:?}", asked.card);
    let plan = json!({"plan": "Do it.", "planFilePath": "/p"});
    let asked = question("ExitPlanMode", Some(&plan)).unwrap();
    assert!(
        matches!(&asked.card, Card::Plan { plan, .. } if plan == "Do it."),
        "{:?}",
        asked.card
    );
    assert!(details("s", &asked.card).contains("Do it."));
    // A call is no card.
    let input = json!({"command": "ls"});
    assert!(question("Bash", Some(&input)).is_none());
    assert!(question("ExitPlanMode", Some(&input)).is_none());
}

#[test]
fn the_whole_view_has_its_keys_first_and_is_bounded() {
    let card = Card::Plan {
        plan: String::new(),
        cut: false,
    };
    let text = "line\n".repeat(super::MAX_WHOLE + 10);
    let shown = whole(&card, &text, 80);
    let rows: Vec<&str> = shown.text.lines().collect();
    assert!(
        said(rows[0]).starts_with("All of it:  y  approve"),
        "{}",
        rows[0]
    );
    assert!(rows[1].contains(" RET  back"), "{}", rows[1]);
    assert_eq!(
        rows.len(),
        super::MAX_WHOLE + 4,
        "the keys, the rows, the cut and the frame's bottom"
    );
    assert!(rows[rows.len() - 2].contains("not shown here"));
    assert!(rows[rows.len() - 1].starts_with("╰──"));
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
