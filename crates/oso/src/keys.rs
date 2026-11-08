//! Translation of the window system's key events into [`KeyPress`], from
//! `oso_platform::window`'s [`KeyInput`], which `app::shell` builds from winit's
//! (ADR 0070).
//!
//! Must not depend on: `commands`, `dispatch` or editor state; it only
//! translates.
//!
//! On macOS, Command is Meta and Option is Super. Elsewhere, Alt is Meta and
//! the logo key is Super (architecture.md, Layers).

use oso_core::key::{Key, KeyPress, Modifiers, NamedKey};
use oso_platform::window::{Held, KeyInput, SystemKey, SystemNamed};

/// The key press for `input`, or `None` for keys oso does not handle yet:
/// lone modifiers, dead keys, text of several characters ([`typed_text`]) and keys
/// with no Emacs name.
pub fn from_event(input: &KeyInput<'_>, held: Held) -> Option<KeyPress> {
    translate(input.key, input.base, held)
}

/// The text of a key that types several characters, such as lam-alef on an Arabic
/// layout, which is no [`KeyPress`]. `None` with Control, Meta or Super held, so
/// composed text with Option on macOS stays out (ADR 0033, ADR 0051).
pub fn typed_text<'a>(input: &KeyInput<'a>, held: Held) -> Option<&'a str> {
    several_characters(input.key, held)
}

fn several_characters(key: SystemKey<'_>, held: Held) -> Option<&str> {
    let SystemKey::Character(text) = key else {
        return None;
    };
    let plain = modifiers(held).without(Modifiers::SHIFT).is_empty();
    let several = text.chars().nth(1).is_some();
    (plain && several && !text.chars().any(char::is_control)).then_some(text)
}

/// Maps the modifiers held to oso's, applying the macOS mapping.
fn modifiers(held: Held) -> Modifiers {
    let (meta, sup) = if cfg!(target_os = "macos") {
        (Held::LOGO, Held::ALT)
    } else {
        (Held::ALT, Held::LOGO)
    };
    [
        (Held::CONTROL, Modifiers::CONTROL),
        (meta, Modifiers::META),
        (Held::SHIFT, Modifiers::SHIFT),
        (sup, Modifiers::SUPER),
    ]
    .into_iter()
    .filter(|(system, _)| held.contains(*system))
    .fold(Modifiers::NONE, |all, (_, oso)| all | oso)
}

/// Translates a key, given as the window system reports it (`key`) and as it would
/// be without modifiers (`base`).
///
/// Characters come from `key`, which already includes Shift (`A`, `!`).
/// With Option held on macOS, or when Control turned the character into a
/// control character, they come from `base` instead, so that Option-a is
/// `s-a` rather than `s-å`. A base letter gets Shift folded back in.
fn translate(key: SystemKey<'_>, base: SystemKey<'_>, held: Held) -> Option<KeyPress> {
    let modifiers = modifiers(held);
    let key = match key {
        SystemKey::Named(named) => Key::Named(match named {
            SystemNamed::Enter => NamedKey::Return,
            SystemNamed::Tab => NamedKey::Tab,
            SystemNamed::Escape => NamedKey::Escape,
            SystemNamed::Backspace => NamedKey::Backspace,
            SystemNamed::Delete => NamedKey::Delete,
            SystemNamed::Insert => NamedKey::Insert,
            SystemNamed::Left => NamedKey::Left,
            SystemNamed::Right => NamedKey::Right,
            SystemNamed::Up => NamedKey::Up,
            SystemNamed::Down => NamedKey::Down,
            SystemNamed::Home => NamedKey::Home,
            SystemNamed::End => NamedKey::End,
            SystemNamed::PageUp => NamedKey::PageUp,
            SystemNamed::PageDown => NamedKey::PageDown,
            SystemNamed::Space => {
                return KeyPress::new(modifiers.without(Modifiers::SHIFT), Key::Char(' '));
            }
            SystemNamed::Function(number) => NamedKey::function(number)?,
            SystemNamed::Other => return None,
        }),
        SystemKey::Character(text) => {
            // Composed text with more than one character is not a key.
            let c = single_char(text)?;
            let composed = cfg!(target_os = "macos") && held.contains(Held::ALT);
            if !c.is_control() && !composed {
                // Shift is already part of the character.
                return KeyPress::new(modifiers.without(Modifiers::SHIFT), Key::char(c)?);
            }
            match base {
                SystemKey::Character(base) => Key::char(single_char(base)?)?,
                _ => return None,
            }
        }
        SystemKey::Unidentified | SystemKey::Dead => return None,
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

#[cfg(test)]
mod tests {
    use super::{Held, SystemKey, SystemNamed, several_characters, translate};
    use oso_core::key::KeyPress;

    fn ch(s: &str) -> SystemKey<'_> {
        SystemKey::Character(s)
    }

    fn press(s: &str) -> Option<KeyPress> {
        Some(s.parse().unwrap())
    }

    /// The key held for Meta: Command on macOS, Alt elsewhere.
    fn meta() -> Held {
        if cfg!(target_os = "macos") {
            Held::LOGO
        } else {
            Held::ALT
        }
    }

    const NONE: Held = Held::NONE;
    const SHIFT: Held = Held::SHIFT;
    const CONTROL: Held = Held::CONTROL;
    const LOGO: Held = Held::LOGO;

    #[test]
    fn only_plain_text_of_several_characters_is_typed_text() {
        let lam_alef = ch("لا");
        assert_eq!(several_characters(lam_alef, NONE), Some("لا"));
        assert_eq!(several_characters(lam_alef, SHIFT), Some("لا"));
        for held in [CONTROL, meta(), LOGO, Held::ALT] {
            assert_eq!(several_characters(lam_alef, held), None, "{held:?}");
        }
        assert_eq!(several_characters(ch("a"), NONE), None, "a key press");
        assert_eq!(several_characters(ch("a\u{7f}"), NONE), None, "a control");
        let enter = SystemKey::Named(SystemNamed::Enter);
        assert_eq!(several_characters(enter, NONE), None);
    }

    #[test]
    fn plain_keys() {
        assert_eq!(translate(ch("a"), ch("a"), NONE), press("a"));
        let right = SystemKey::Named(SystemNamed::Right);
        assert_eq!(translate(right, right, NONE), press("<right>"));
        let enter = SystemKey::Named(SystemNamed::Enter);
        assert_eq!(translate(enter, enter, NONE), press("RET"));
        let f12 = SystemKey::Named(SystemNamed::Function(12));
        assert_eq!(translate(f12, f12, NONE), press("<f12>"));
        let space = SystemKey::Named(SystemNamed::Space);
        assert_eq!(translate(space, space, SHIFT), press("SPC"));
    }

    #[test]
    fn shift_is_part_of_the_character() {
        assert_eq!(translate(ch("A"), ch("a"), SHIFT), press("A"));
        assert_eq!(translate(ch("!"), ch("1"), SHIFT), press("!"));
        let left = SystemKey::Named(SystemNamed::Left);
        assert_eq!(translate(left, left, SHIFT), press("S-<left>"));
    }

    #[test]
    fn control_characters_fall_back_to_the_base_key() {
        assert_eq!(translate(ch("\u{18}"), ch("x"), CONTROL), press("C-x"));
        assert_eq!(translate(ch("x"), ch("x"), CONTROL), press("C-x"));
        assert_eq!(
            translate(ch("\u{18}"), ch("x"), CONTROL | SHIFT),
            press("C-X")
        );
    }

    #[test]
    fn meta_and_super_follow_the_platform() {
        assert_eq!(translate(ch("f"), ch("f"), meta()), press("M-f"));
        if cfg!(target_os = "macos") {
            // Option composes characters; the base key is used instead.
            let option = Held::ALT;
            assert_eq!(translate(ch("å"), ch("a"), option), press("s-a"));
            assert_eq!(translate(ch("Å"), ch("a"), option | SHIFT), press("s-A"));
        } else {
            let logo = Held::LOGO;
            assert_eq!(translate(ch("a"), ch("a"), logo), press("s-a"));
            // Alt does not compose characters here, so Shift still applies.
            assert_eq!(translate(ch("!"), ch("1"), meta() | SHIFT), press("M-!"));
        }
    }

    #[test]
    fn unhandled_keys_are_none() {
        let shift = SystemKey::Named(SystemNamed::Other);
        assert_eq!(translate(shift, shift, SHIFT), None);
        let dead = SystemKey::Dead;
        assert_eq!(translate(dead, ch("`"), NONE), None);
        let unknown = SystemKey::Unidentified;
        assert_eq!(translate(unknown, unknown, NONE), None);
        assert_eq!(translate(ch("ab"), ch("a"), NONE), None);
    }
}
