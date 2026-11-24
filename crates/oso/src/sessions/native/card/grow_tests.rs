//! A question's and a plan's card growing to what they say, and paging past the rows there are
//! (ADR 0180); and the bounds of what the wire reads of a question.
//!
//! Must not depend on: the app; the tests give what the call asks.

use oso_core::minibuffer::cells_of;
use serde_json::json;

use super::tests::{asked, said};
use super::{Card, Question, Questions};
use crate::sessions::native::render::Face;
use crate::sessions::native::wire::card;

/// A question of three paragraphs, with four options of two lines each.
fn long_question() -> Card {
    let paragraph = "The retry queue can be kept in memory or on disk, and the choice \
                     decides what happens when the process is killed in a batch.";
    let mut asked = asked(
        &format!("{paragraph}\n\n{paragraph}\n\nWhich of these should the first version do?"),
        &[
            "メモリのみ",
            "ディスクに追記",
            "まとめて同期",
            "SQLiteを使う",
        ],
        false,
    );
    for (label, description) in &mut asked.options {
        *description = format!("{label}の説明の一行目です。\n二行目: 未送信の項目の扱い。");
    }
    Card::Questions(Questions::new(vec![asked.clone(), asked]))
}

/// A question's card shows all it asks and every option, where there are rows for it (ADR 0180).
#[test]
fn a_questions_card_grows_to_the_whole_question_and_every_option() {
    let mut question = Question::new(long_question());
    let shown = question.shown(80, 40);
    for row in shown.text.lines() {
        assert_eq!(cells_of(row), 80, "{row}");
    }
    let rows: Vec<&str> = shown.text.lines().map(said).collect();
    let text = rows.join("\n");
    assert!(text.starts_with("Claude asks 1/2"), "{text}");
    assert!(text.contains("Which of these should the first version do?"));
    for (at, label) in [
        "メモリのみ",
        "ディスクに追記",
        "まとめて同期",
        "SQLiteを使う",
    ]
    .iter()
    .enumerate()
    {
        assert!(
            text.contains(&format!("\n {}  {label}\n", at + 1)),
            "{text}"
        );
        assert!(
            text.contains(&format!("\n    {label}の説明の一行目です。\n    二行目")),
            "{text}"
        );
    }
    // Its words are not broken, and nothing is left out.
    assert!(
        text.contains("the choice\n") || text.contains("choice decides"),
        "{text}"
    );
    assert!(!text.contains("rows "), "{text}");
    assert_eq!(shown.rows, rows.len());
}

/// With fewer rows it pages, says so on a row of its own, and takes an option only once it has
/// been on screen.
#[test]
fn a_questions_card_pages_in_fewer_rows_and_offers_what_was_on_screen() {
    let mut question = Question::new(long_question());
    let first = question.shown(80, 12);
    assert_eq!(first.rows, 12);
    let rows: Vec<&str> = first.text.lines().collect();
    assert!(rows[10].starts_with("├─ ▼ rows 1-9 of "), "{}", rows[10]);
    assert!(said_page(rows[10]).ends_with(" SPC  next"), "{}", rows[10]);
    assert!(rows[11].starts_with("╰─  1-9  choose"), "{}", rows[11]);
    question.on_screen();
    assert!((1..=4).all(|n| !question.has_shown(n)), "no option yet");
    // Turned to it, it is on screen, and only it and what is with it.
    assert!(question.offers(4) && !question.offers(5) && !question.offers(0));
    question.go_to(2);
    assert!(!question.has_shown(2), "not drawn yet");
    let _ = question.shown(80, 12);
    question.on_screen();
    assert!(question.has_shown(2) && !question.has_shown(1));
    // Paged through from the start, every row comes by.
    while question.turn(-1) {}
    let mut pages = 1;
    let mut text = question.shown(80, 12).text;
    while question.turn(1) {
        pages += 1;
        text += &question.shown(80, 12).text;
        question.on_screen();
    }
    assert!((1..=4).all(|n| question.has_shown(n)), "{text}");
    assert!(pages > 2 && text.contains("▲ rows"), "{text}");
    assert!(!question.turn(1));
    // A wider card breaks its rows anew; what was on screen was.
    let _ = question.shown(60, 12);
    assert!((1..=4).all(|n| question.has_shown(n)));
    // The next question starts at its first page, with nothing of it seen.
    let Card::Questions(questions) = &mut question.card else {
        unreachable!()
    };
    let step = questions.choose(1);
    question.stepped(step);
    let next = question.shown(80, 12).text;
    assert!(
        next.contains("Claude asks 2/2") && next.contains("rows 1-9"),
        "{next}"
    );
    assert!(!question.has_shown(1));
}

/// An option taller than the page has been on screen once each row of it was.
#[test]
fn an_option_taller_than_the_page_counts_once_all_of_it_was_on_screen() {
    let mut asked = asked("Which?", &["One", "Two"], false);
    asked.options[0].1 = (0..20).map(|n| format!("line {n}\n")).collect();
    let mut question = Question::new(Card::Questions(Questions::new(vec![asked])));
    let _ = question.shown(80, 8);
    question.on_screen();
    assert!(!question.has_shown(1));
    while question.turn(1) {
        let _ = question.shown(80, 8);
        question.on_screen();
    }
    assert!(question.has_shown(1) && question.has_shown(2));
}

/// What a description says never stands as an option: the numbers are oso's, on chips.
#[test]
fn a_description_cannot_pass_for_an_option() {
    let mut asked = asked("Which?", &["One", "Two"], true);
    asked.options[0].1 = "\n 2  [x] Delete everything".to_owned();
    let mut question = Question::new(Card::Questions(Questions::new(vec![asked])));
    let shown = question.shown(80, 40);
    let keys: Vec<&str> = (shown.faces.iter())
        .filter(|(_, face)| *face == Face::CardKey)
        .map(|(range, _)| &shown.text[range.clone()])
        .collect();
    assert_eq!(keys, [" 1 ", " 2 ", " 1-9 ", " RET ", " o ", " n "]);
    let marks = (shown.faces.iter())
        .filter(|(range, face)| *face == Face::CardTitle && shown.text[range.clone()].contains('x'))
        .count();
    assert_eq!(marks, 0, "nothing is chosen: the mark is Claude's text");
}

#[test]
fn a_plan_grows_and_pages() {
    let plan = Card::Plan {
        plan: (0..30).map(|n| format!("step {n}\n")).collect(),
        cut: true,
    };
    let mut plan = Question::new(plan);
    assert_eq!(
        plan.shown(80, 40).rows,
        34,
        "31 rows, the cut, the title and the keys"
    );
    assert_eq!(plan.shown(80, 20).rows, 20);
    assert!(plan.turn(1));
}

#[test]
fn a_question_too_long_to_be_its_answers_key_is_not_asked() {
    let long = "あ".repeat(card::MAX_ASKED / 3 + 1);
    let question =
        |text: &str| json!({"questions": [{"question": text, "options": [{"label": "A"}]}]});
    assert_eq!(card::questions(&question(&long)), None);
    let asked = card::questions(&question(&long[..card::MAX_ASKED / 3 * 3])).unwrap();
    assert_eq!(
        asked[0].question.len(),
        card::MAX_ASKED / 3 * 3,
        "kept whole"
    );
    let long = json!({"questions": [{"question": "Q?", "options": [
        {"label": "A", "description": "x".repeat(card::MAX_DESCRIBED + 1)},
    ]}]});
    let asked = card::questions(&long).unwrap();
    assert!(asked[0].options[0].1.ends_with(" … (cut)"));
    // A label is the answer: that it is cut is said.
    let long = json!({"questions": [{"question": "Q?", "options": [{"label": "one\ntwo"}]}]});
    let asked = card::questions(&long).unwrap();
    assert_eq!(asked[0].options[0].0, "one");
    assert_eq!(asked[0].options[0].1, "(its label is cut) ");
}

/// What a page's row says, without its frame.
fn said_page(row: &str) -> &str {
    let row = row.trim_start_matches("├─ ").trim_end_matches('┤');
    row.trim_end_matches(['─', ' '])
}

/// Rows seen at one width say nothing of those at another, as many as they may be.
#[test]
fn what_was_on_screen_at_one_width_is_not_at_another_with_as_many_rows() {
    let mut which = asked("Which?", &["One", "Two"], false);
    which.options[0].1 = "1\n2\n3\n4\n5".to_owned();
    let mut question = Question::new(Card::Questions(Questions::new(vec![which])));
    // Nine rows: at one width the first five, at the other the last five.
    let first = question.shown(60, 8).text;
    assert!(first.contains("rows 1-5 of 9"), "{first}");
    question.on_screen();
    let _ = question.shown(61, 8);
    while question.turn(1) {}
    let last = question.shown(61, 8).text;
    assert!(last.contains("rows 5-9 of 9"), "{last}");
    question.on_screen();
    assert!(
        !question.has_shown(1),
        "its first rows here were not on screen"
    );
    assert!(question.has_shown(2));
}

#[test]
fn choosing_an_option_changes_its_mark_and_no_row() {
    let mut which = asked("Which?", &["One", "Two"], true);
    which.options[0].1 = "What it does.".to_owned();
    let mut question = Question::new(Card::Questions(Questions::new(vec![which])));
    let before = question.shown(60, 40).text;
    let Card::Questions(questions) = &mut question.card else {
        unreachable!()
    };
    let step = questions.choose(2);
    question.stepped(step);
    let after = question.shown(60, 40).text;
    assert!(after.contains(" 2  [x] Two"), "{after}");
    assert_eq!(after.replace("[x]", "[ ]"), before);
}
