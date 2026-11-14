//! Mouse reports per mode and format, as libghostty-vt's encoder makes them from oso's
//! pointers (ADR 0120, step 4b-2).
//!
//! Must not depend on: the policy or the thread.

use oso_core::key::Modifiers;

use super::super::Ghostty;
use super::super::tests::engine;
use crate::engine::Engine;
use crate::limits;
use crate::snapshot::{Button, Pointer, PointerAction};

/// A pointer at `column` and `row` of the tests' 8 × 16 pixel cells.
fn at(action: PointerAction, column: u16, row: u16) -> Pointer {
    Pointer {
        action,
        column,
        row,
        x: u32::from(column) * 8 + 3,
        y: u32::from(row) * 16 + 5,
        modifiers: Modifiers::NONE,
    }
}

fn encode(e: &mut Ghostty, pointer: Pointer, held: Option<Button>) -> Vec<u8> {
    let mut out = [0; limits::KEY_BYTES_MAX];
    let n = e.encode_pointer(pointer, held, &mut out);
    out[..n].to_vec()
}

fn press(e: &mut Ghostty, column: u16, row: u16) -> Vec<u8> {
    let p = at(PointerAction::Press(Button::Left), column, row);
    encode(e, p, Some(Button::Left))
}

fn release(e: &mut Ghostty, column: u16, row: u16) -> Vec<u8> {
    encode(
        e,
        at(PointerAction::Release(Button::Left), column, row),
        None,
    )
}

#[test]
fn nothing_is_encoded_with_reports_off() {
    let mut e = engine(80, 24);
    assert_eq!(press(&mut e, 1, 1), b"");
    assert_eq!(release(&mut e, 1, 1), b"");
    assert_eq!(encode(&mut e, at(PointerAction::Motion, 2, 1), None), b"");
    let wheel = at(PointerAction::Press(Button::WheelUp), 1, 1);
    assert_eq!(encode(&mut e, wheel, None), b"");
}

#[test]
fn presses_and_releases_in_1000_and_x10s_format() {
    let mut e = engine(80, 24);
    e.feed(b"\x1b[?1000h");
    assert_eq!(press(&mut e, 0, 0), b"\x1b[M !!");
    assert_eq!(release(&mut e, 0, 0), b"\x1b[M#!!");
    // Motion is not reported in 1000.
    let moved = at(PointerAction::Motion, 3, 0);
    assert_eq!(encode(&mut e, moved, Some(Button::Left)), b"");
}

#[test]
fn x10_mode_9_reports_presses_only() {
    let mut e = engine(80, 24);
    e.feed(b"\x1b[?9h");
    assert_eq!(press(&mut e, 2, 1), b"\x1b[M #\"");
    assert_eq!(release(&mut e, 2, 1), b"");
}

#[test]
fn x10s_format_cannot_say_a_column_past_222() {
    let mut e = engine(400, 24);
    e.feed(b"\x1b[?1000h");
    assert_eq!(press(&mut e, 300, 0), b"");
    e.feed(b"\x1b[?1006h");
    assert_eq!(press(&mut e, 300, 0), b"\x1b[<0;301;1M");
}

#[test]
fn sgr_1006_presses_releases_and_the_wheel() {
    let mut e = engine(80, 24);
    e.feed(b"\x1b[?1000h\x1b[?1006h");
    assert_eq!(press(&mut e, 4, 2), b"\x1b[<0;5;3M");
    assert_eq!(release(&mut e, 4, 2), b"\x1b[<0;5;3m");
    let right = at(PointerAction::Press(Button::Right), 4, 2);
    assert_eq!(encode(&mut e, right, Some(Button::Right)), b"\x1b[<2;5;3M");
    let middle = at(PointerAction::Press(Button::Middle), 4, 2);
    assert_eq!(
        encode(&mut e, middle, Some(Button::Middle)),
        b"\x1b[<1;5;3M"
    );
    let notches = [
        (Button::WheelUp, &b"\x1b[<64;5;3M"[..]),
        (Button::WheelDown, b"\x1b[<65;5;3M"),
        (Button::WheelLeft, b"\x1b[<66;5;3M"),
        (Button::WheelRight, b"\x1b[<67;5;3M"),
    ];
    for (button, sent) in notches {
        let wheel = at(PointerAction::Press(button), 4, 2);
        assert_eq!(encode(&mut e, wheel, None), sent, "{button:?}");
    }
}

#[test]
fn control_is_reported_with_the_button() {
    let mut e = engine(80, 24);
    e.feed(b"\x1b[?1000h\x1b[?1006h");
    let mut p = at(PointerAction::Press(Button::Left), 0, 0);
    p.modifiers = Modifiers::CONTROL;
    assert_eq!(encode(&mut e, p, Some(Button::Left)), b"\x1b[<16;1;1M");
}

#[test]
fn drags_in_1002_and_every_motion_in_1003() {
    let mut e = engine(80, 24);
    e.feed(b"\x1b[?1002h\x1b[?1006h");
    let moved = at(PointerAction::Motion, 5, 2);
    assert_eq!(encode(&mut e, moved, Some(Button::Left)), b"\x1b[<32;6;3M");
    assert_eq!(encode(&mut e, moved, None), b"", "no button held");
    e.feed(b"\x1b[?1003h");
    assert_eq!(encode(&mut e, moved, None), b"\x1b[<35;6;3M");
}

#[test]
fn the_other_formats_1005_1015_and_1016() {
    let mut e = engine(400, 24);
    e.feed(b"\x1b[?1000h\x1b[?1005h");
    // UTF-8: column 300 is 300 + 33, two bytes.
    assert_eq!(press(&mut e, 300, 0), "\x1b[M \u{14d}!".as_bytes());
    e.feed(b"\x1b[?1005l\x1b[?1015h");
    assert_eq!(press(&mut e, 4, 2), b"\x1b[32;5;3M");
    e.feed(b"\x1b[?1015l\x1b[?1016h");
    // Pixels, kept inside the cell.
    let sent = String::from_utf8(press(&mut e, 4, 2)).expect("text");
    assert_eq!(sent, "\x1b[<0;35;37M");
}

#[test]
fn the_pixel_is_kept_inside_the_cell_the_ui_found() {
    let mut e = engine(80, 24);
    e.feed(b"\x1b[?1000h\x1b[?1006h");
    let mut p = at(PointerAction::Press(Button::Left), 4, 2);
    p.x = 1000;
    p.y = 0;
    assert_eq!(encode(&mut e, p, Some(Button::Left)), b"\x1b[<0;5;3M");
}
