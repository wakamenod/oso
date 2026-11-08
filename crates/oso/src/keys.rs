//! Translation of the window system's key events into [`KeyPress`], from oso's own
//! [`KeyInput`], which `app::shell` builds from winit's (ADR 0070).
//!
//! Must not depend on: `commands`, `dispatch` or editor state; it only
//! translates.
//!
//! On macOS, Command is Meta and Option is Super. Elsewhere, Alt is Meta and
//! the logo key is Super (architecture.md, Layers).

use std::ops::BitOr;

use oso_core::key::{Key, KeyPress, Modifiers, NamedKey};

/// A key press as the window system reports it, in oso's own types: only real presses
/// ([`is_real_press`]) become one. The text it borrows lives as long as the event. No
/// `Debug`, as it holds what the person types.
#[derive(Clone, Copy)]
pub struct KeyInput<'a> {
    /// The key with the modifiers and the layout applied: `A` with Shift, `å` with Option.
    pub key: SystemKey<'a>,
    /// The key as it would be without modifiers, but for Shift on some systems.
    pub base: SystemKey<'a>,
    /// Held down, and sent again by the system.
    pub repeat: bool,
}

/// A key as the window system names it. `Debug` only in tests, as a character is what the
/// person types.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub enum SystemKey<'a> {
    Named(SystemNamed),
    /// The text the key types, usually one character.
    Character(&'a str),
    /// A dead key, which waits for the next one to compose.
    Dead,
    /// A key the system gave no name.
    Unidentified,
}

/// The named keys oso tells apart. The rest, lone modifiers among them, are `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemNamed {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    /// A function key and its number, such as 12 for F12.
    Function(u8),
    Other,
}

/// The modifier keys held, by the names the window system gives them: on macOS Alt is
/// Option and Logo is Command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held(u8);

impl Held {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1);
    pub const CONTROL: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const LOGO: Self = Self(1 << 3);

    /// Whether every key in `other` is held.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Held {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Whether a key event is a press the user made now: `pressed`, and not `synthetic`. On
/// X11 and Windows, winit generates presses for keys already held when the window gains
/// focus, and one of those would run a command or type text nobody asked for.
pub fn is_real_press(pressed: bool, synthetic: bool) -> bool {
    pressed && !synthetic
}

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
    use super::{Held, SystemKey, SystemNamed, is_real_press, several_characters, translate};
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
    fn only_a_real_press_counts() {
        assert!(is_real_press(true, false));
        assert!(!is_real_press(true, true), "held at focus");
        assert!(!is_real_press(false, false));
        assert!(!is_real_press(false, true));
    }

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
