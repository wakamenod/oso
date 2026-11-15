//! Tests of the new instance's side of `oso-restart`, with a fake UI thread.
//!
//! Must not depend on: processes or windows.

use std::cell::{Cell, RefCell};

use serde_json::{Map, json};

use super::{Coming, piece};
use crate::carry::pair::{Message, STAGE_PIECE, TAKE_MASTERS};
use crate::carry::{Asked, Op};

/// What the fake UI thread was asked, and the generation its imports report.
#[derive(Default)]
struct Ui {
    asked: RefCell<Vec<String>>,
}

impl Ui {
    fn post(&self) -> impl Fn(Asked) -> bool + '_ {
        move |asked| {
            match asked {
                Asked::Op { op, passed, reply } => {
                    let name = match &op {
                        Op::ImportText(_) => "import-text".to_owned(),
                        Op::ImportState(_) => "import-state".to_owned(),
                        Op::ImportTerminals { count } => {
                            format!("terminals {count:?} with {}", passed.brought.len())
                        }
                        other => format!("{other:?}"),
                    };
                    self.asked.borrow_mut().push(name);
                    let _ = reply.send(Ok(json!({ "generation": 4 })));
                }
                Asked::PreviousStays { generation } => {
                    self.asked.borrow_mut().push(format!("left {generation:?}"));
                }
                other => self.asked.borrow_mut().push(format!("{other:?}")),
            }
            true
        }
    }
}

fn message(say: &str, fields: serde_json::Value) -> Message {
    let serde_json::Value::Object(fields) = fields else {
        unreachable!()
    };
    Message {
        say: say.into(),
        fields,
        fds: Vec::new(),
    }
}

#[test]
fn a_piece_names_its_place_and_text() {
    let fields = json!({ "export": 1, "item": 0, "offset": 3, "text": "ab" });
    let serde_json::Value::Object(fields) = fields else {
        unreachable!()
    };
    let carried = piece(&fields).unwrap();
    assert_eq!((carried.export, carried.item, carried.offset), (1, 0, 3));
    assert_eq!(carried.text, "ab");
    assert!(piece(&Map::new()).is_err());
}

#[test]
fn pieces_go_to_the_ui_and_masters_before_a_state_are_refused() {
    let ui = Ui::default();
    let mut coming = Coming {
        post: ui.post(),
        generation: None,
        terminals: 0,
        masters: Vec::new(),
    };
    let fields = json!({ "export": 1, "item": 0, "offset": 0, "text": "ab" });
    assert!(coming.answer(message(STAGE_PIECE, fields)).is_ok());
    assert!(
        coming
            .answer(message(TAKE_MASTERS, json!({ "count": 0 })))
            .is_err()
    );
    assert!(coming.answer(message("export-state", json!({}))).is_err());
    assert_eq!(*ui.asked.borrow(), ["import-text"]);
}

#[test]
fn held_masters_are_taken_only_once_the_parent_has_gone() {
    let ui = Ui::default();
    let (_reader, writer) = std::io::pipe().unwrap();
    let coming = Coming {
        post: ui.post(),
        generation: Some(4),
        terminals: 1,
        masters: vec![writer.into()],
    };
    let looks = Cell::new(0);
    coming.ended(10, || {
        looks.set(looks.get() + 1);
        if looks.get() < 3 { 10 } else { 1 }
    });
    assert_eq!(looks.get(), 3);
    assert_eq!(*ui.asked.borrow(), ["terminals Some(1) with 1"]);
}

#[test]
fn masters_are_given_up_if_none_came_or_when_told_to_stay() {
    let ui = Ui::default();
    let coming = Coming {
        post: ui.post(),
        generation: Some(4),
        terminals: 2,
        masters: Vec::new(),
    };
    coming.ended(10, || 1);
    let mut coming = Coming {
        post: ui.post(),
        generation: Some(4),
        terminals: 2,
        masters: Vec::new(),
    };
    coming.left();
    assert_eq!(
        *ui.asked.borrow(),
        ["terminals None with 0", "left Some(4)"]
    );
}
