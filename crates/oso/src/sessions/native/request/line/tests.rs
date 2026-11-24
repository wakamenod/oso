//! The line's text (ADR 0182): its keys and what it says at the right, at every width, and which
//! keys are lit.
//!
//! Must not depend on: the app; the tests give a block and what of it was read.

use std::time::Instant;

use serde_json::json;

use super::*;
use crate::sessions::native::request::reading::{READ_AFTER, Row, View};

const HERE: Says = Says {
    here: true,
    answers: true,
};

/// A block of `tool` whose body has `lines` lines, and what of it was read: its first `read` lines,
/// while its first `shown` are in view.
fn set_up(tool: &str, lines: usize, read: usize, shown: usize) -> (Block, Reading) {
    let input = json!({"command": vec!["a"; lines].join("\n")});
    let later = Instant::now() + READ_AFTER * 100;
    let block = Block::call(tool, Some(&input), later);
    let mut reading = Reading::new(&block);
    let ends: Vec<usize> = (block.body().text.match_indices('\n').map(|(at, _)| at + 1)).collect();
    let view = |lines: usize| View {
        window: None,
        columns: 80,
        wraps: true,
        attended: true,
        rows: (0..lines.min(ends.len()))
            .map(|at| Row {
                start: at.checked_sub(1).map_or(0, |before| ends[before]),
                end: ends[at],
                at,
                cells: 3,
            })
            .collect(),
    };
    let now = Instant::now();
    if read > 0 {
        reading.look(&view(read), true, now);
        reading.look(&view(read), true, now + READ_AFTER);
    }
    reading.look(&view(shown), true, now + READ_AFTER);
    (block, reading)
}

fn text(set_up: &(Block, Reading), says: Says, width: usize) -> String {
    line(&set_up.0, &set_up.1, says, width).shown.text
}

/// The words on chips.
fn lit(line: &Line) -> Vec<&str> {
    (line.shown.faces.iter())
        .filter(|(_, face)| *face == Face::CardKey)
        .map(|(range, _)| line.shown.text[range.clone()].trim())
        .collect()
}

#[test]
fn a_line_says_the_keys_and_what_was_read() {
    let all = set_up("Bash", 3, 3, 3);
    assert_eq!(
        text(&all, HERE, 90),
        " y  allow   n  deny   N  deny, saying why   RET  its input   i  write      Bash · all read"
    );
    assert_eq!(
        text(&all, HERE, 60),
        concat!(
            " y  allow   n  deny   N  why   RET  input   i  write\n",
            "                                             Bash · all read"
        )
    );
    assert_eq!(
        text(&all, HERE, 72),
        concat!(
            " y  allow   n  deny   N  why   RET  input   i  write\n",
            "                                                         Bash · all read"
        )
    );
    let some = set_up("Edit", 201, 34, 34);
    assert_eq!(
        text(&some, HERE, 81),
        " SPC  read on   n  deny   N  why   C-u y  unread   i  write   34 of 202 rows read"
    );
    let elsewhere = Says {
        here: false,
        ..HERE
    };
    assert_eq!(
        text(&all, elsewhere, 60),
        " Bash asks to run   C-c C-j  answers it"
    );
    assert_eq!(
        text(&all, elsewhere, 38),
        " Bash asks to run   C-c C-j  answers"
    );
    assert_eq!(
        text(&all, elsewhere, 30),
        " Bash asks to run\n C-c C-j  answers it"
    );
    assert_eq!(text(&all, HERE, 19), "Bash asks to run");
}

#[test]
fn what_a_line_shows_is_what_a_key_does_now() {
    let shown = |set_up: &(Block, Reading), says| {
        let line = line(&set_up.0, &set_up.1, says, 120);
        let keys = line.keys;
        (keys, lit(&line).join(" "), line.shown.text.clone())
    };
    let all = set_up("Bash", 3, 3, 3);
    assert_eq!(shown(&all, HERE).0, Keys::Allow);
    assert_eq!(shown(&all, HERE).1, "y n N RET i");
    // While an answer would be refused, none is lit.
    let waits = Says {
        answers: false,
        ..HERE
    };
    assert_eq!(shown(&all, waits).1, "");
    assert!(shown(&all, waits).2.starts_with(" y  allow   n  deny"));
    // All of it in view, and not read yet: `y` would not allow.
    let soon = set_up("Bash", 3, 0, 3);
    assert_eq!(shown(&soon, HERE).0, Keys::Allow);
    assert_eq!(shown(&soon, HERE).1, "n N RET i");
    assert!(shown(&soon, HERE).2.ends_with("  Bash"));
    // Where the view is not attended, the same keys, dim.
    let mut away = set_up("Bash", 3, 3, 3);
    let mut unattended = View {
        attended: false,
        ..View::default()
    };
    unattended.columns = 80;
    unattended.wraps = true;
    unattended.rows = vec![Row {
        start: 8,
        end: 12,
        at: 2,
        cells: 3,
    }];
    away.1.look(&unattended, false, Instant::now());
    assert_eq!(shown(&away, waits).0, Keys::Allow);
    assert_eq!(shown(&away, waits).1, "");
    // Read, and its end out of view: `y` is not shown.
    let away = set_up("Bash", 3, 3, 2);
    assert_eq!(shown(&away, HERE).0, Keys::ToEnd);
    assert_eq!(shown(&away, HERE).1, "SPC n N RET i");
    assert!(shown(&away, HERE).2.ends_with("Bash · all read"));
    let some = set_up("Bash", 30, 10, 10);
    assert_eq!(shown(&some, HERE).0, Keys::ReadOn);
    assert_eq!(shown(&some, HERE).1, "SPC n N C-u y i");
    // Nothing of it was in view for long enough to ask about.
    let unseen = set_up("Bash", 30, 0, 10);
    assert_eq!(shown(&unseen, HERE).1, "SPC n N i");
    // What is not shown is allowed only unread.
    let long = set_up("Bash", 2002, 2001, 2001);
    assert_eq!(shown(&long, HERE).0, Keys::Unread);
    assert_eq!(shown(&long, HERE).1, "C-u y n N RET i");
    assert!(shown(&long, HERE).2.ends_with("2 lines not shown"));
    let long = set_up("Bash", 2002, 20, 20);
    assert_eq!(shown(&long, HERE).0, Keys::ReadOn);
    assert!(shown(&long, HERE).2.ends_with("20 of 2002 rows read"));
    let none = set_up("Bash", 40 << 10, 1, 1);
    assert_eq!(shown(&none, HERE).0, Keys::Unread);
    assert!(shown(&none, HERE).2.ends_with("not shown at all"));
    // Elsewhere, only the way there, whatever waits.
    let elsewhere = Says {
        here: false,
        answers: false,
    };
    assert_eq!(shown(&all, elsewhere).1, "C-c C-j");
}

#[test]
fn a_line_fits_any_width_and_leaves_no_key_out() {
    let odd = "mcp__\u{202e}server__".repeat(8);
    for (tool, lines, read, shown) in [
        ("Bash", 3, 3, 3),
        ("Bash", 3, 0, 3),
        (&odd, 3, 3, 1),
        (&odd, 202, 34, 34),
        ("Edit", 2002, 2001, 2001),
        ("Bash", 40 << 10, 1, 1),
    ] {
        let (block, reading) = set_up(tool, lines, read, shown);
        for width in 0..=160 {
            for says in [
                HERE,
                Says {
                    answers: false,
                    ..HERE
                },
                Says {
                    here: false,
                    ..HERE
                },
            ] {
                let line = line(&block, &reading, says, width);
                let text = &line.shown.text;
                let rows: Vec<&str> = text.split('\n').collect();
                assert_eq!(rows.len(), line.rows, "{width}");
                assert!(!text.ends_with('\n'));
                for row in &rows {
                    assert!(cells_of(row) <= width.max(1), "{width}: {row:?}");
                }
                if width < NARROWEST {
                    assert_eq!((line.keys, lit(&line).len()), (Keys::Narrow, 0));
                    continue;
                }
                for key in line.keys.keys() {
                    let (key, ..) = key.words();
                    assert!(
                        text.contains(&format!(" {key} ")),
                        "{width}: {key} in {text:?}"
                    );
                }
                // Every byte is oso's, and in one of its faces.
                let mut at = 0;
                for (range, face) in &line.shown.faces {
                    assert_eq!(range.start, at);
                    assert!(matches!(face, Face::CardKey | Face::CardTitle | Face::Dim));
                    at = range.end;
                }
                assert_eq!(at, text.len());
                assert!(
                    text.chars().all(|c| c.is_ascii() || "·…\n".contains(c)),
                    "{text:?}"
                );
            }
        }
    }
}
