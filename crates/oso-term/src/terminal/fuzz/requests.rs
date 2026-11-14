//! The UI's requests the fuzz target interleaves with output (ADR 0120, step 4): keys with
//! any modifiers, pastes of crafted text with and without lines allowed, the mouse's pointers
//! anywhere, scrolls by any amount and focus changes. Each is made from the input's generator,
//! and written to and read from a script line (`key`, `paste`, `pointer`, `scroll`, `focus`).
//! What a paste wrote is checked here.
//!
//! Must not depend on: the engine; `fuzz` serves what this makes, as the thread does.

use std::fmt::Write as _;

use oso_core::key::{Key, KeyPress, Modifiers, NamedKey};

use super::Rng;
use crate::snapshot::Scroll;
use crate::snapshot::{Button, Pointer, PointerAction};
use crate::terminal::{Input, Request};

const CHARS: &[char] = &[
    'a',
    'Z',
    '1',
    '!',
    ' ',
    '~',
    '\\',
    '[',
    'あ',
    'é',
    '\u{1F600}',
    '\u{FFFF}',
    '\u{10FFFF}',
];
const NAMED: &[NamedKey] = &[
    NamedKey::Return,
    NamedKey::Tab,
    NamedKey::Escape,
    NamedKey::Backspace,
    NamedKey::Delete,
    NamedKey::Insert,
    NamedKey::Left,
    NamedKey::Right,
    NamedKey::Up,
    NamedKey::Down,
    NamedKey::Home,
    NamedKey::End,
    NamedKey::PageUp,
    NamedKey::PageDown,
];
const PASTED: &[&str] = &[
    "ls",
    "\n",
    "\r\n",
    "\r",
    "\t",
    "\x1b[201~",
    "\x1b[200~",
    "\u{9b}201~",
    "\x00",
    "\x7f",
    "\u{85}",
    "日本",
    "e\u{301}",
    " ",
    "\x1b",
];
const LINES: &[isize] = &[
    0,
    1,
    -1,
    -3,
    24,
    -24,
    -100_000,
    100_000,
    isize::MIN,
    isize::MAX,
];

/// Up to three requests, as the UI might send them between two reads.
pub(super) fn requests(rng: &mut Rng) -> Vec<Request> {
    (0..rng.below(4))
        .filter_map(|_| match rng.below(9) {
            0..=2 => key(rng).map(|key| Request::Input(Input::Key(key))),
            3 | 4 => Some(Request::Input(Input::Paste {
                text: paste(rng),
                lines_allowed: rng.chance(50),
            })),
            5 => Some(Request::Input(Input::Focus(rng.chance(50)))),
            6 => Some(Request::Input(Input::Pointer(pointer(rng)))),
            7 => Some(Request::Scroll(Scroll::Lines(*rng.pick(LINES)))),
            _ => Some(Request::Scroll(Scroll::Bottom)),
        })
        .collect()
}

fn key(rng: &mut Rng) -> Option<KeyPress> {
    let mut modifiers = Modifiers::NONE;
    for modifier in [
        Modifiers::CONTROL,
        Modifiers::META,
        Modifiers::SUPER,
        Modifiers::SHIFT,
    ] {
        if rng.chance(25) {
            modifiers = modifiers | modifier;
        }
    }
    let key = match rng.below(4) {
        0 => Key::Char(char::from(b' ' + rng.below(95) as u8)),
        1 => Key::Char(*rng.pick(CHARS)),
        2 => Key::Named(NamedKey::function(1 + rng.below(35) as u8)?),
        _ => Key::Named(*rng.pick(NAMED)),
    };
    KeyPress::new(modifiers, key)
}

const BUTTONS: &[Button] = &[
    Button::Left,
    Button::Middle,
    Button::Right,
    Button::WheelUp,
    Button::WheelDown,
    Button::WheelLeft,
    Button::WheelRight,
];
/// The modifiers a pointer's script line names by bit, in this order.
const MODIFIERS: [Modifiers; 4] = [
    Modifiers::CONTROL,
    Modifiers::META,
    Modifiers::SUPER,
    Modifiers::SHIFT,
];
const PLACES: &[u32] = &[0, 1, 7, 79, 80, 222, 223, 999, 1000, 65_535, u32::MAX];

/// A pointer anywhere, inside the screen or far past it.
fn pointer(rng: &mut Rng) -> Pointer {
    let action = match rng.below(3) {
        0 => PointerAction::Press(*rng.pick(BUTTONS)),
        1 => PointerAction::Release(*rng.pick(BUTTONS)),
        _ => PointerAction::Motion,
    };
    let place = |rng: &mut Rng| {
        if rng.chance(50) {
            *rng.pick(PLACES)
        } else {
            rng.below(100) as u32
        }
    };
    let column = u16::try_from(place(rng)).unwrap_or(u16::MAX);
    let row = u16::try_from(place(rng)).unwrap_or(u16::MAX);
    let (x, y) = (place(rng), place(rng));
    let modifiers = MODIFIERS
        .iter()
        .filter(|_| rng.chance(25))
        .fold(Modifiers::NONE, |all, &m| all | m);
    Pointer {
        action,
        column,
        row,
        x,
        y,
        modifiers,
    }
}

fn paste(rng: &mut Rng) -> String {
    let pieces = if rng.chance(3) { 5000 } else { 12 };
    (0..rng.below(pieces)).map(|_| *rng.pick(PASTED)).collect()
}

/// `request` as a script line.
pub(super) fn line(request: &Request) -> String {
    match request {
        Request::Input(Input::Key(key)) => format!("key {key}"),
        Request::Input(Input::Paste {
            text,
            lines_allowed,
        }) => {
            let mut line = format!("paste {} x", u8::from(*lines_allowed));
            for byte in text.bytes() {
                let _ = write!(line, "{byte:02x}");
            }
            line
        }
        Request::Input(Input::Focus(focused)) => format!("focus {}", u8::from(*focused)),
        Request::Input(Input::Pointer(p)) => {
            let (action, button) = match p.action {
                PointerAction::Press(button) => ("press", Some(button)),
                PointerAction::Release(button) => ("release", Some(button)),
                PointerAction::Motion => ("motion", None),
            };
            let button = button.map_or(0, |b| BUTTONS.iter().position(|&o| o == b).unwrap_or(0));
            let mods = (MODIFIERS.iter().enumerate())
                .filter(|(_, m)| p.modifiers.contains(**m))
                .fold(0, |all, (bit, _)| all | 1 << bit);
            let (column, row, x, y) = (p.column, p.row, p.x, p.y);
            format!("pointer {action} {button} {column} {row} {x} {y} {mods}")
        }
        Request::Scroll(Scroll::Lines(lines)) => format!("scroll {lines}"),
        Request::Scroll(Scroll::Bottom) => "scroll bottom".to_owned(),
        _ => String::new(),
    }
}

/// The request a script line names, or `None` for a line of another kind.
pub(super) fn parse(words: &[&str]) -> Option<Request> {
    let request = match words {
        ["key", key] => Request::Input(Input::Key(key.parse().expect("a key"))),
        ["paste", allowed, hex] => {
            let hex = hex.strip_prefix('x').expect("x and hex");
            let bytes: Vec<u8> = (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex"))
                .collect();
            Request::Input(Input::Paste {
                text: String::from_utf8(bytes).expect("UTF-8"),
                lines_allowed: *allowed == "1",
            })
        }
        ["focus", focused] => Request::Input(Input::Focus(*focused == "1")),
        ["pointer", action, button, column, row, x, y, mods] => {
            let button = BUTTONS[button.parse::<usize>().expect("a button")];
            let action = match *action {
                "press" => PointerAction::Press(button),
                "release" => PointerAction::Release(button),
                _ => PointerAction::Motion,
            };
            let mods: u8 = mods.parse().expect("modifiers");
            let modifiers = (MODIFIERS.iter().enumerate())
                .filter(|(bit, _)| mods & 1 << bit != 0)
                .fold(Modifiers::NONE, |all, (_, &m)| all | m);
            Request::Input(Input::Pointer(Pointer {
                action,
                column: column.parse().expect("column"),
                row: row.parse().expect("row"),
                x: x.parse().expect("x"),
                y: y.parse().expect("y"),
                modifiers,
            }))
        }
        ["scroll", "bottom"] => Request::Scroll(Scroll::Bottom),
        ["scroll", lines] => Request::Scroll(Scroll::Lines(lines.parse().expect("lines"))),
        _ => return None,
    };
    Some(request)
}

/// What any paste may write: no ESC or other control but TAB, LF and CR inside its brackets.
pub(super) fn check_paste(bytes: &[u8]) {
    let inner = bytes
        .strip_prefix(b"\x1b[200~")
        .and_then(|b| b.strip_suffix(b"\x1b[201~"))
        .unwrap_or(bytes);
    let text = std::str::from_utf8(inner).expect("UTF-8");
    let control = |c: char| c.is_control() && !matches!(c, '\t' | '\n' | '\r');
    assert!(!text.chars().any(control), "{bytes:?}");
}

/// A script of the smoke run's inputs runs as they did.
#[test]
fn a_script_replays_its_inputs() {
    super::script::run_script(&super::script::script_of(0x5eed, 40, true));
}
