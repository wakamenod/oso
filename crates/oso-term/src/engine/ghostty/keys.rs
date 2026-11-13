//! oso's key presses as libghostty-vt's key events (ADR 0120's Input). A `KeyPress` already
//! holds Shift in its character, so a character is sent as its text with the key and the
//! unshifted character it has on a US layout, Shift marked as used for the text; a character
//! outside ASCII has no key, only its text. Named keys keep Shift. Meta, Command on macOS, is
//! sent as Alt; Super as the library's super.
//!
//! Must not depend on: the policy or the thread. This file is safe code over `ffi`'s
//! constants.

use oso_core::key::{Key, KeyPress, Modifiers, NamedKey};

use super::ffi;
use super::raw::KeyInput;

/// US-layout characters typed with Shift, with the character of their key without it.
const SHIFTED: [(char, char); 21] = [
    ('~', '`'),
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
];

/// The key event for `press`.
pub(super) fn input(press: KeyPress) -> KeyInput {
    const MODS: [(Modifiers, ffi::Mods); 4] = [
        (Modifiers::CONTROL, ffi::MODS_CTRL),
        (Modifiers::META, ffi::MODS_ALT),
        (Modifiers::SUPER, ffi::MODS_SUPER),
        (Modifiers::SHIFT, ffi::MODS_SHIFT),
    ];
    let mods = MODS
        .iter()
        .filter(|(modifier, _)| press.modifiers.contains(*modifier))
        .fold(0, |all, (_, bit)| all | bit);
    let mut input = KeyInput {
        key: ffi::KEY_UNIDENTIFIED,
        mods,
        consumed: 0,
        text: [0; 4],
        text_len: 0,
        unshifted: 0,
    };
    match press.key {
        Key::Named(named) => input.key = named_key(named),
        Key::Char(c) => {
            input.text_len = c.encode_utf8(&mut input.text).len();
            let (key, unshifted) = us_key(c).unwrap_or((ffi::KEY_UNIDENTIFIED, c));
            input.key = key;
            input.unshifted = u32::from(unshifted);
            if unshifted != c {
                input.mods |= ffi::MODS_SHIFT;
                input.consumed = ffi::MODS_SHIFT;
            }
        }
    }
    input
}

fn named_key(named: NamedKey) -> ffi::Key {
    match named {
        NamedKey::Return => ffi::KEY_ENTER,
        NamedKey::Tab => ffi::KEY_TAB,
        NamedKey::Escape => ffi::KEY_ESCAPE,
        NamedKey::Backspace => ffi::KEY_BACKSPACE,
        NamedKey::Delete => ffi::KEY_DELETE,
        NamedKey::Insert => ffi::KEY_INSERT,
        NamedKey::Left => ffi::KEY_ARROW_LEFT,
        NamedKey::Right => ffi::KEY_ARROW_RIGHT,
        NamedKey::Up => ffi::KEY_ARROW_UP,
        NamedKey::Down => ffi::KEY_ARROW_DOWN,
        NamedKey::Home => ffi::KEY_HOME,
        NamedKey::End => ffi::KEY_END,
        NamedKey::PageUp => ffi::KEY_PAGE_UP,
        NamedKey::PageDown => ffi::KEY_PAGE_DOWN,
        // The library knows F1 to F25; a higher one sends nothing.
        NamedKey::F(n @ 1..=ffi::KEY_F_LAST) => ffi::KEY_F1 + ffi::Key::from(n - 1),
        NamedKey::F(_) => ffi::KEY_UNIDENTIFIED,
    }
}

/// The US-layout key of an ASCII character, and the character that key types without
/// Shift; `None` outside ASCII.
fn us_key(c: char) -> Option<(ffi::Key, char)> {
    let base = SHIFTED
        .iter()
        .find(|(shifted, _)| *shifted == c)
        .map_or(c.to_ascii_lowercase(), |&(_, base)| base);
    let offset = |from: char| ffi::Key::from(base as u8 - from as u8);
    let key = match base {
        'a'..='z' => ffi::KEY_A + offset('a'),
        '0'..='9' => ffi::KEY_DIGIT_0 + offset('0'),
        ' ' => ffi::KEY_SPACE,
        '`' => ffi::KEY_BACKQUOTE,
        '\\' => ffi::KEY_BACKSLASH,
        '[' => ffi::KEY_BRACKET_LEFT,
        ']' => ffi::KEY_BRACKET_RIGHT,
        ',' => ffi::KEY_COMMA,
        '=' => ffi::KEY_EQUAL,
        '-' => ffi::KEY_MINUS,
        '.' => ffi::KEY_PERIOD,
        '\'' => ffi::KEY_QUOTE,
        ';' => ffi::KEY_SEMICOLON,
        '/' => ffi::KEY_SLASH,
        _ => return None,
    };
    Some((key, base))
}

#[cfg(test)]
mod tests;
