//! Translation of winit key events into [`KeyPress`].
//!
//! On macOS, Command is Meta and Option is Super. Elsewhere, Alt is Meta and
//! the logo key is Super (architecture.md, Layers).

use oso_core::key::{Key, KeyPress, Modifiers, NamedKey};
use winit::event::KeyEvent;
use winit::keyboard::{self, ModifiersState};

/// The key press for `event`, or `None` for keys oso does not handle yet:
/// lone modifiers, dead keys, composed text and keys with no Emacs name.
pub fn from_event(event: &KeyEvent, modifiers: ModifiersState) -> Option<KeyPress> {
    translate(&event.logical_key, &key_without_modifiers(event), modifiers)
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn key_without_modifiers(event: &KeyEvent) -> keyboard::Key {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    event.key_without_modifiers()
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn key_without_modifiers(event: &KeyEvent) -> keyboard::Key {
    event.logical_key.clone()
}

/// Maps winit's modifier state to oso's, applying the macOS mapping.
fn modifiers(state: ModifiersState) -> Modifiers {
    let (meta, sup) = if cfg!(target_os = "macos") {
        (ModifiersState::SUPER, ModifiersState::ALT)
    } else {
        (ModifiersState::ALT, ModifiersState::SUPER)
    };
    [
        (ModifiersState::CONTROL, Modifiers::CONTROL),
        (meta, Modifiers::META),
        (ModifiersState::SHIFT, Modifiers::SHIFT),
        (sup, Modifiers::SUPER),
    ]
    .into_iter()
    .filter(|(winit, _)| state.contains(*winit))
    .fold(Modifiers::NONE, |all, (_, oso)| all | oso)
}

/// Translates a key, given as winit reports it (`logical`) and as it would
/// be without modifiers (`base`).
///
/// Characters come from `logical`, which already includes Shift (`A`, `!`).
/// With Option held on macOS, or when Control turned the character into a
/// control character, they come from `base` instead, so that Option-a is
/// `s-a` rather than `s-å`. A base letter gets Shift folded back in.
fn translate(
    logical: &keyboard::Key,
    base: &keyboard::Key,
    state: ModifiersState,
) -> Option<KeyPress> {
    use keyboard::{Key as K, NamedKey as N};

    let modifiers = modifiers(state);
    let key = match logical {
        K::Named(named) => Key::Named(match named {
            N::Enter => NamedKey::Return,
            N::Tab => NamedKey::Tab,
            N::Escape => NamedKey::Escape,
            N::Backspace => NamedKey::Backspace,
            N::Delete => NamedKey::Delete,
            N::Insert => NamedKey::Insert,
            N::ArrowLeft => NamedKey::Left,
            N::ArrowRight => NamedKey::Right,
            N::ArrowUp => NamedKey::Up,
            N::ArrowDown => NamedKey::Down,
            N::Home => NamedKey::Home,
            N::End => NamedKey::End,
            N::PageUp => NamedKey::PageUp,
            N::PageDown => NamedKey::PageDown,
            N::Space => return KeyPress::new(modifiers.without(Modifiers::SHIFT), Key::Char(' ')),
            other => NamedKey::function(function_number(*other)?)?,
        }),
        K::Character(text) => {
            // Composed text with more than one character is not a key.
            let c = single_char(text)?;
            let composed = cfg!(target_os = "macos") && state.contains(ModifiersState::ALT);
            if !c.is_control() && !composed {
                // Shift is already part of the character.
                return KeyPress::new(modifiers.without(Modifiers::SHIFT), Key::char(c)?);
            }
            match base {
                K::Character(base) => Key::char(single_char(base)?)?,
                _ => return None,
            }
        }
        K::Unidentified(_) | K::Dead(_) => return None,
    };
    // A base character with Shift: letters become upper case, and other
    // characters drop Shift, since the shifted symbol depends on the layout.
    match key {
        Key::Char(c) if !c.is_ascii_alphabetic() => {
            KeyPress::new(modifiers.without(Modifiers::SHIFT), key)
        }
        _ => KeyPress::new(modifiers, key),
    }
}

/// The text's only character, if it has exactly one.
fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// The number of a function key, such as 12 for F12.
fn function_number(key: keyboard::NamedKey) -> Option<u8> {
    use keyboard::NamedKey as N;
    const KEYS: [keyboard::NamedKey; 35] = [
        N::F1,
        N::F2,
        N::F3,
        N::F4,
        N::F5,
        N::F6,
        N::F7,
        N::F8,
        N::F9,
        N::F10,
        N::F11,
        N::F12,
        N::F13,
        N::F14,
        N::F15,
        N::F16,
        N::F17,
        N::F18,
        N::F19,
        N::F20,
        N::F21,
        N::F22,
        N::F23,
        N::F24,
        N::F25,
        N::F26,
        N::F27,
        N::F28,
        N::F29,
        N::F30,
        N::F31,
        N::F32,
        N::F33,
        N::F34,
        N::F35,
    ];
    let index = KEYS.iter().position(|k| *k == key)?;
    u8::try_from(index + 1).ok()
}

#[cfg(test)]
mod tests {
    use super::translate;
    use oso_core::key::KeyPress;
    use winit::keyboard::{Key, ModifiersState, NamedKey, NativeKey};

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    fn press(s: &str) -> Option<KeyPress> {
        Some(s.parse().unwrap())
    }

    /// The winit state for Meta: Command on macOS, Alt elsewhere.
    fn meta() -> ModifiersState {
        if cfg!(target_os = "macos") {
            ModifiersState::SUPER
        } else {
            ModifiersState::ALT
        }
    }

    const NONE: ModifiersState = ModifiersState::empty();
    const SHIFT: ModifiersState = ModifiersState::SHIFT;
    const CONTROL: ModifiersState = ModifiersState::CONTROL;

    #[test]
    fn plain_keys() {
        assert_eq!(translate(&ch("a"), &ch("a"), NONE), press("a"));
        let right = Key::Named(NamedKey::ArrowRight);
        assert_eq!(translate(&right, &right, NONE), press("<right>"));
        let enter = Key::Named(NamedKey::Enter);
        assert_eq!(translate(&enter, &enter, NONE), press("RET"));
        let f12 = Key::Named(NamedKey::F12);
        assert_eq!(translate(&f12, &f12, NONE), press("<f12>"));
        let space = Key::Named(NamedKey::Space);
        assert_eq!(translate(&space, &space, SHIFT), press("SPC"));
    }

    #[test]
    fn shift_is_part_of_the_character() {
        assert_eq!(translate(&ch("A"), &ch("a"), SHIFT), press("A"));
        assert_eq!(translate(&ch("!"), &ch("1"), SHIFT), press("!"));
        let left = Key::Named(NamedKey::ArrowLeft);
        assert_eq!(translate(&left, &left, SHIFT), press("S-<left>"));
    }

    #[test]
    fn control_characters_fall_back_to_the_base_key() {
        assert_eq!(translate(&ch("\u{18}"), &ch("x"), CONTROL), press("C-x"));
        assert_eq!(translate(&ch("x"), &ch("x"), CONTROL), press("C-x"));
        assert_eq!(
            translate(&ch("\u{18}"), &ch("x"), CONTROL | SHIFT),
            press("C-X")
        );
    }

    #[test]
    fn meta_and_super_follow_the_platform() {
        assert_eq!(translate(&ch("f"), &ch("f"), meta()), press("M-f"));
        if cfg!(target_os = "macos") {
            // Option composes characters; the base key is used instead.
            let option = ModifiersState::ALT;
            assert_eq!(translate(&ch("å"), &ch("a"), option), press("s-a"));
            assert_eq!(translate(&ch("Å"), &ch("a"), option | SHIFT), press("s-A"));
        } else {
            let logo = ModifiersState::SUPER;
            assert_eq!(translate(&ch("a"), &ch("a"), logo), press("s-a"));
            // Alt does not compose characters here, so Shift still applies.
            assert_eq!(translate(&ch("!"), &ch("1"), meta() | SHIFT), press("M-!"));
        }
    }

    #[test]
    fn unhandled_keys_are_none() {
        let shift = Key::Named(NamedKey::Shift);
        assert_eq!(translate(&shift, &shift, SHIFT), None);
        let dead = Key::Dead(Some('`'));
        assert_eq!(translate(&dead, &ch("`"), NONE), None);
        let unknown = Key::Unidentified(NativeKey::Unidentified);
        assert_eq!(translate(&unknown, &unknown, NONE), None);
        assert_eq!(translate(&ch("ab"), &ch("a"), NONE), None);
    }
}
