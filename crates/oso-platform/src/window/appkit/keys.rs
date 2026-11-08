//! What an AppKit key event says, as `window`'s types: the named keys by their virtual key
//! code, the modifiers held, and ranges the input method gives in UTF-16 as UTF-8 bytes.
//! Plain functions, tested without a window.
//!
//! Must not depend on: the view or the run loop; nothing here calls AppKit.

use std::ops::Range;

use objc2_app_kit::NSEventModifierFlags;

use crate::window::{Held, SystemNamed};

/// The named key for a virtual key code (Carbon's `kVK_` codes, which do not depend on the
/// layout), or `None` for a key that types a character.
pub(super) fn named(code: u16) -> Option<SystemNamed> {
    use SystemNamed as N;
    Some(match code {
        0x24 | 0x4C => N::Enter,
        0x30 => N::Tab,
        0x31 => N::Space,
        0x33 => N::Backspace,
        0x35 => N::Escape,
        0x75 => N::Delete,
        0x72 => N::Insert,
        0x7B => N::Left,
        0x7C => N::Right,
        0x7D => N::Down,
        0x7E => N::Up,
        0x73 => N::Home,
        0x77 => N::End,
        0x74 => N::PageUp,
        0x79 => N::PageDown,
        // Lone modifiers and Caps Lock, which reach `keyDown:` only on some keyboards.
        0x36..=0x3F => N::Other,
        _ => return function_number(code).map(N::Function),
    })
}

/// The number of a function key, F1 to F20, by its virtual key code.
fn function_number(code: u16) -> Option<u8> {
    const CODES: [u16; 20] = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71,
        0x6A, 0x40, 0x4F, 0x50, 0x5A,
    ];
    let index = CODES.iter().position(|c| *c == code)?;
    u8::try_from(index + 1).ok()
}

/// The modifiers an event says are held, by AppKit's names: Option is `ALT` and Command is
/// `LOGO`.
pub(super) fn held(flags: NSEventModifierFlags) -> Held {
    [
        (NSEventModifierFlags::Shift, Held::SHIFT),
        (NSEventModifierFlags::Control, Held::CONTROL),
        (NSEventModifierFlags::Option, Held::ALT),
        (NSEventModifierFlags::Command, Held::LOGO),
    ]
    .into_iter()
    .filter(|(flag, _)| flags.contains(*flag))
    .fold(Held::NONE, |all, (_, held)| all | held)
}

/// The UTF-8 byte range of `text` that a range of UTF-16 code units names, cut to the text
/// and to character boundaries. An input method sends these, and another process's numbers
/// are not trusted.
pub(super) fn utf8_range(text: &str, utf16: Range<usize>) -> Range<usize> {
    let byte_at = |units: usize| {
        let mut counted = 0;
        for (byte, c) in text.char_indices() {
            if counted >= units {
                return byte;
            }
            counted += c.len_utf16();
        }
        text.len()
    };
    let start = byte_at(utf16.start);
    start..byte_at(utf16.end).max(start)
}

/// How many UTF-16 code units `text` has, as AppKit counts ranges.
pub(super) fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::{held, named, utf8_range, utf16_len};
    use crate::window::{Held, SystemNamed};
    use objc2_app_kit::NSEventModifierFlags as F;

    #[test]
    fn named_keys_go_by_their_code_and_letters_by_their_text() {
        assert_eq!(named(0x24), Some(SystemNamed::Enter));
        assert_eq!(named(0x4C), Some(SystemNamed::Enter), "keypad Enter");
        assert_eq!(named(0x75), Some(SystemNamed::Delete));
        assert_eq!(named(0x7B), Some(SystemNamed::Left));
        assert_eq!(named(0x7A), Some(SystemNamed::Function(1)));
        assert_eq!(named(0x5A), Some(SystemNamed::Function(20)));
        assert_eq!(named(0x38), Some(SystemNamed::Other), "Shift");
        assert_eq!(named(0x00), None, "the A key types");
    }

    #[test]
    fn modifiers_keep_appkits_names() {
        assert_eq!(held(F::empty()), Held::NONE);
        assert_eq!(held(F::Command | F::Shift), Held::LOGO | Held::SHIFT);
        assert_eq!(held(F::Option | F::Control), Held::ALT | Held::CONTROL);
        assert_eq!(held(F::CapsLock | F::Function), Held::NONE);
    }

    #[test]
    fn utf16_ranges_become_byte_ranges_on_boundaries() {
        assert_eq!(utf8_range("かな", 1..2), 3..6);
        assert_eq!(utf8_range("かな", 2..2), 6..6, "the caret at the end");
        assert_eq!(utf8_range("a😀b", 1..3), 1..5, "a surrogate pair");
        assert_eq!(
            utf8_range("a😀b", 2..3),
            5..5,
            "inside a pair, cut to a boundary"
        );
        assert_eq!(utf8_range("ab", 5..9), 2..2, "past the end");
        let (start, end) = (2, 1);
        assert_eq!(utf8_range("ab", start..end), 2..2, "reversed");
        assert_eq!(utf16_len("a😀か"), 4);
    }
}
