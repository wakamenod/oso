//! Key presses and Emacs key notation.
//!
//! Must not depend on: winit or other window-system types, `keymap` or
//! `command`.
//!
//! A [`KeyPress`] is one key with its modifiers, independent of the window
//! system. The tip crate translates OS key events into it, and the dev control
//! socket parses it from Emacs notation (`"C-x C-f"`, `"<right>"`, `"RET"`),
//! so both reach the editor through the same path. Keymaps build on it.
//!
//! # Notation
//!
//! - Keys in a sequence are separated by ASCII whitespace.
//! - Modifier prefixes: `C-` Control, `M-` Meta, `S-` Shift, `s-` Super.
//!   They may come in any order when parsed and are printed in that order.
//! - Characters stand for themselves (`a`, `A`, `!`, `あ`), except `SPC`.
//! - Named keys: `RET`, `TAB`, `ESC`, `DEL` (Backspace), `<delete>`,
//!   `<insert>`, `<left>`, `<right>`, `<up>`, `<down>`, `<home>`, `<end>`,
//!   `<prior>` (Page Up), `<next>` (Page Down) and `<f1>` to `<f35>`. The
//!   aliases `<return>`, `<tab>`, `<escape>`, `<backspace>` and
//!   `<deletechar>` are accepted and printed in the short form.
//!
//! # Shift
//!
//! For a character key, Shift is part of the character: Shift and `a` is
//! `A`, Shift and `1` is `!` on a US layout. So [`Modifiers::SHIFT`] is only
//! kept on named keys (`S-<right>`). When parsing, `S-` on an ASCII letter
//! gives the upper-case letter (`C-S-a` is `C-A`), and `S-` on any other
//! character is an error, since the shifted character depends on the layout.

use std::fmt;
use std::ops::BitOr;
use std::str::FromStr;

/// A set of modifier keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    /// No modifiers.
    pub const NONE: Self = Self(0);
    /// Control (`C-`).
    pub const CONTROL: Self = Self(1);
    /// Meta (`M-`). Command on macOS, Alt elsewhere.
    pub const META: Self = Self(1 << 1);
    /// Shift (`S-`). Only kept on named keys; see the module docs.
    pub const SHIFT: Self = Self(1 << 2);
    /// Super (`s-`). Option on macOS, the logo key elsewhere.
    pub const SUPER: Self = Self(1 << 3);

    /// Prefixes in the order they are printed, which is the order Emacs uses.
    const PREFIXES: [(Self, &'static str); 4] = [
        (Self::CONTROL, "C-"),
        (Self::META, "M-"),
        (Self::SHIFT, "S-"),
        (Self::SUPER, "s-"),
    ];

    /// Whether every modifier in `other` is in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// `self` without the modifiers in `other`.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// A key that is not a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    /// `RET`
    Return,
    /// `TAB`
    Tab,
    /// `ESC`
    Escape,
    /// `DEL`: the key that deletes backward.
    Backspace,
    /// `<delete>`: the key that deletes forward.
    Delete,
    /// `<insert>`
    Insert,
    /// `<left>`
    Left,
    /// `<right>`
    Right,
    /// `<up>`
    Up,
    /// `<down>`
    Down,
    /// `<home>`
    Home,
    /// `<end>`
    End,
    /// `<prior>`
    PageUp,
    /// `<next>`
    PageDown,
    /// `<f1>` to `<f35>`. Construct it through [`NamedKey::function`].
    F(u8),
}

impl NamedKey {
    /// The highest function key number.
    pub const MAX_FUNCTION: u8 = 35;

    /// Function key `n`, or `None` unless `1 <= n <= 35`.
    pub const fn function(n: u8) -> Option<Self> {
        if n >= 1 && n <= Self::MAX_FUNCTION {
            Some(Self::F(n))
        } else {
            None
        }
    }

    /// Keys printed with a fixed name, with their names. Aliases come after
    /// the printed name.
    const NAMES: [(Self, &'static str); 19] = [
        (Self::Return, "RET"),
        (Self::Return, "<return>"),
        (Self::Tab, "TAB"),
        (Self::Tab, "<tab>"),
        (Self::Escape, "ESC"),
        (Self::Escape, "<escape>"),
        (Self::Backspace, "DEL"),
        (Self::Backspace, "<backspace>"),
        (Self::Delete, "<delete>"),
        (Self::Delete, "<deletechar>"),
        (Self::Insert, "<insert>"),
        (Self::Left, "<left>"),
        (Self::Right, "<right>"),
        (Self::Up, "<up>"),
        (Self::Down, "<down>"),
        (Self::Home, "<home>"),
        (Self::End, "<end>"),
        (Self::PageUp, "<prior>"),
        (Self::PageDown, "<next>"),
    ];

    fn from_name(name: &str) -> Option<Self> {
        if let Some((key, _)) = Self::NAMES.iter().find(|(_, n)| *n == name) {
            return Some(*key);
        }
        let digits = name.strip_prefix("<f")?.strip_suffix('>')?;
        // Reject `<f01>` and `<f+1>`, which `parse` would accept.
        if digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Self::function(digits.parse().ok()?)
    }
}

impl fmt::Display for NamedKey {
    #[expect(
        clippy::expect_used,
        reason = "NAMES lists every named key except F, which returns above"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::F(n) = self {
            return write!(f, "<f{n}>");
        }
        let (_, name) = Self::NAMES
            .iter()
            .find(|(key, _)| key == self)
            .expect("every named key but F has a name");
        f.write_str(name)
    }
}

/// A key without its modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// A character, as typed: `a`, `A`, `!`, ` ` (`SPC`). Construct it
    /// through [`Key::char`].
    Char(char),
    /// A key that is not a character.
    Named(NamedKey),
}

impl Key {
    /// The key for `c`, or `None` if `c` cannot be a key: a control
    /// character or ASCII whitespace other than space. Tab and Return are
    /// [`NamedKey`]s.
    pub fn char(c: char) -> Option<Self> {
        is_key_char(c).then_some(Self::Char(c))
    }
}

/// Whether `c` can be a [`Key::Char`]. ASCII whitespace separates keys in
/// the notation, so only space is allowed, written `SPC`.
fn is_key_char(c: char) -> bool {
    !c.is_control() && (c == ' ' || !c.is_ascii_whitespace())
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Char(' ') => f.write_str("SPC"),
            Self::Char(c) => write!(f, "{c}"),
            Self::Named(named) => named.fmt(f),
        }
    }
}

/// One key press: a key and the modifiers held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyPress {
    /// Held modifiers. Never contains [`Modifiers::SHIFT`] for a
    /// [`Key::Char`].
    pub modifiers: Modifiers,
    /// The key.
    pub key: Key,
}

impl KeyPress {
    /// A key press with no modifiers.
    pub const fn plain(key: Key) -> Self {
        Self {
            modifiers: Modifiers::NONE,
            key,
        }
    }

    /// A key press, with Shift folded into a character as described in the
    /// module docs. Returns `None` for Shift on a character other than an
    /// ASCII letter, or for a character that cannot be a key.
    pub fn new(modifiers: Modifiers, key: Key) -> Option<Self> {
        let key = match key {
            Key::Char(c) if !is_key_char(c) => return None,
            Key::Char(c) if modifiers.contains(Modifiers::SHIFT) => {
                if !c.is_ascii_alphabetic() {
                    return None;
                }
                Key::Char(c.to_ascii_uppercase())
            }
            Key::Named(NamedKey::F(n)) if NamedKey::function(n).is_none() => return None,
            key => key,
        };
        let modifiers = match key {
            Key::Char(_) => modifiers.without(Modifiers::SHIFT),
            Key::Named(_) => modifiers,
        };
        Some(Self { modifiers, key })
    }
}

impl fmt::Display for KeyPress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (modifier, prefix) in Modifiers::PREFIXES {
            if self.modifiers.contains(modifier) {
                f.write_str(prefix)?;
            }
        }
        self.key.fmt(f)
    }
}

/// A key or key sequence that is not valid Emacs notation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseKeyError {
    token: String,
    reason: &'static str,
}

impl fmt::Display for ParseKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid key {:?}: {}", self.token, self.reason)
    }
}

impl std::error::Error for ParseKeyError {}

/// The most characters of a bad token that a [`ParseKeyError`] keeps, so
/// that errors stay short whatever the input.
const MAX_ERROR_TOKEN: usize = 32;

impl FromStr for KeyPress {
    type Err = ParseKeyError;

    /// Parses one key, such as `C-x`, `M-S-<left>` or `SPC`.
    fn from_str(token: &str) -> Result<Self, ParseKeyError> {
        let error = |reason| {
            let mut shown: String = token.chars().take(MAX_ERROR_TOKEN).collect();
            if shown.len() < token.len() {
                shown.push('…');
            }
            ParseKeyError {
                token: shown,
                reason,
            }
        };
        let mut modifiers = Modifiers::NONE;
        let mut rest = token;
        // A prefix counts only when something follows it, so `C-` alone and
        // the `-` in `C--` are keys rather than modifiers.
        'prefixes: loop {
            for (modifier, prefix) in Modifiers::PREFIXES {
                if let Some(after) = rest.strip_prefix(prefix)
                    && !after.is_empty()
                {
                    if modifiers.contains(modifier) {
                        return Err(error("repeated modifier"));
                    }
                    modifiers = modifiers | modifier;
                    rest = after;
                    continue 'prefixes;
                }
            }
            break;
        }

        let key = if rest == "SPC" {
            Key::Char(' ')
        } else if let Some(named) = NamedKey::from_name(rest) {
            Key::Named(named)
        } else {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Key::char(c).ok_or_else(|| error("not a key character"))?,
                (None, _) => return Err(error("empty key")),
                (Some(_), Some(_)) => return Err(error("unknown key name")),
            }
        };
        Self::new(modifiers, key).ok_or_else(|| {
            error("S- works only with letters and named keys; type the shifted character instead")
        })
    }
}

/// Keys pressed one after another, such as `C-x C-f`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct KeySequence(pub Vec<KeyPress>);

impl FromStr for KeySequence {
    type Err = ParseKeyError;

    /// Parses keys separated by ASCII whitespace. An empty string is an
    /// empty sequence.
    fn from_str(s: &str) -> Result<Self, ParseKeyError> {
        s.split_ascii_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map(Self)
    }
}

impl fmt::Display for KeySequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, key) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            key.fmt(f)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Key, KeyPress, KeySequence, Modifiers, NamedKey};
    use proptest::prelude::*;

    fn press(s: &str) -> KeyPress {
        s.parse().unwrap_or_else(|e| panic!("{e}"))
    }

    fn ch(c: char) -> Key {
        Key::char(c).unwrap()
    }

    #[test]
    fn modifiers_combine_as_a_set() {
        assert!(Modifiers::NONE.is_empty());
        assert!(!Modifiers::CONTROL.is_empty());
        let both = Modifiers::CONTROL | Modifiers::META;
        assert!(both.contains(Modifiers::META));
        assert_eq!(both | Modifiers::META, both, "not a toggle");
        assert_eq!(both.without(Modifiers::META), Modifiers::CONTROL);
    }

    // --- Parsing ---

    #[test]
    fn parses_plain_characters() {
        assert_eq!(press("a"), KeyPress::plain(ch('a')));
        assert_eq!(press("A"), KeyPress::plain(ch('A')));
        assert_eq!(press("あ"), KeyPress::plain(ch('あ')));
        assert_eq!(press("<"), KeyPress::plain(ch('<')));
        assert_eq!(press("-"), KeyPress::plain(ch('-')));
    }

    #[test]
    fn parses_modifiers_in_any_order() {
        let expected = KeyPress {
            modifiers: Modifiers::CONTROL | Modifiers::META,
            key: ch('x'),
        };
        assert_eq!(press("C-M-x"), expected);
        assert_eq!(press("M-C-x"), expected);
    }

    #[test]
    fn parses_every_modifier() {
        let all = Modifiers::CONTROL | Modifiers::META | Modifiers::SHIFT | Modifiers::SUPER;
        assert_eq!(
            press("s-S-M-C-<up>"),
            KeyPress {
                modifiers: all,
                key: Key::Named(NamedKey::Up),
            }
        );
    }

    #[test]
    fn a_trailing_dash_is_the_key() {
        assert_eq!(
            press("C--"),
            KeyPress {
                modifiers: Modifiers::CONTROL,
                key: ch('-'),
            }
        );
        assert_eq!(press("C"), KeyPress::plain(ch('C')));
        assert_eq!(
            press("M-C"),
            KeyPress {
                modifiers: Modifiers::META,
                key: ch('C'),
            }
        );
    }

    #[test]
    fn parses_named_keys_and_aliases() {
        assert_eq!(press("RET"), press("<return>"));
        assert_eq!(press("TAB"), press("<tab>"));
        assert_eq!(press("ESC"), press("<escape>"));
        assert_eq!(press("DEL"), press("<backspace>"));
        assert_eq!(press("<delete>"), press("<deletechar>"));
        assert_eq!(press("SPC"), KeyPress::plain(ch(' ')));
        assert_eq!(
            press("<right>"),
            KeyPress::plain(Key::Named(NamedKey::Right))
        );
        assert_eq!(
            press("<prior>"),
            KeyPress::plain(Key::Named(NamedKey::PageUp))
        );
        assert_eq!(press("<f12>"), KeyPress::plain(Key::Named(NamedKey::F(12))));
    }

    #[test]
    fn shift_folds_into_letters() {
        assert_eq!(press("S-a"), press("A"));
        assert_eq!(press("C-S-a"), press("C-A"));
        assert_eq!(press("S-A"), press("A"));
        assert_eq!(
            press("S-<right>"),
            KeyPress {
                modifiers: Modifiers::SHIFT,
                key: Key::Named(NamedKey::Right),
            }
        );
    }

    #[test]
    fn rejects_invalid_keys() {
        for bad in [
            "", "C-", "foo", "<foo>", "<f0>", "<f36>", "<f01>", "<f+1>", "S-1", "S-SPC", "C-C-x",
            "\u{7}", "<RIGHT>", "ret",
        ] {
            assert!(bad.parse::<KeyPress>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn parse_errors_name_the_token() {
        let err = "C-foo".parse::<KeyPress>().unwrap_err();
        assert_eq!(err.to_string(), "invalid key \"C-foo\": unknown key name");
    }

    #[test]
    fn parse_errors_shorten_long_tokens() {
        let long = "x".repeat(10_000);
        let err = long.parse::<KeyPress>().unwrap_err().to_string();
        assert!(err.len() < 100, "{err}");
        assert!(err.contains(&format!("{}…", "x".repeat(32))), "{err}");
    }

    #[test]
    fn parses_sequences() {
        let seq: KeySequence = "C-x  C-f\t<right>\nRET".parse().unwrap();
        assert_eq!(
            seq.0,
            vec![press("C-x"), press("C-f"), press("<right>"), press("RET")]
        );
        assert_eq!(seq.to_string(), "C-x C-f <right> RET");
        assert_eq!("".parse::<KeySequence>().unwrap(), KeySequence::default());
        assert!("C-x foo".parse::<KeySequence>().is_err());
    }

    // --- Printing ---

    #[test]
    fn prints_the_canonical_form() {
        for (input, printed) in [
            ("M-C-x", "C-M-x"),
            ("s-S-<left>", "S-s-<left>"),
            ("<return>", "RET"),
            ("<backspace>", "DEL"),
            ("<deletechar>", "<delete>"),
            ("C-S-a", "C-A"),
            ("SPC", "SPC"),
            ("M-SPC", "M-SPC"),
            ("<f1>", "<f1>"),
        ] {
            assert_eq!(press(input).to_string(), printed, "{input}");
        }
    }

    // --- Constructors ---

    #[test]
    fn constructors_reject_what_cannot_be_printed() {
        assert_eq!(Key::char('\t'), None);
        assert_eq!(Key::char('\n'), None);
        assert_eq!(Key::char('\u{0}'), None);
        assert_eq!(NamedKey::function(0), None);
        assert_eq!(NamedKey::function(36), None);
        assert_eq!(KeyPress::new(Modifiers::SHIFT, Key::Char('1')), None);
        assert_eq!(KeyPress::new(Modifiers::NONE, Key::Char('\t')), None);
        assert_eq!(
            KeyPress::new(Modifiers::NONE, Key::Named(NamedKey::F(0))),
            None
        );
    }

    // --- Property-based tests (proptest) ---

    fn key_char() -> impl Strategy<Value = char> {
        any::<char>().prop_filter_map("not a key character", |c| Key::char(c).map(|_| c))
    }

    fn named_key() -> impl Strategy<Value = NamedKey> {
        prop_oneof![
            prop::sample::select(
                NamedKey::NAMES
                    .iter()
                    .map(|(key, _)| *key)
                    .collect::<Vec<_>>()
            ),
            (1..=NamedKey::MAX_FUNCTION).prop_map(NamedKey::F),
        ]
    }

    /// Any valid key press: Shift appears only on named keys.
    fn key_press() -> impl Strategy<Value = KeyPress> {
        let modifiers = (0u8..16).prop_map(Modifiers);
        let key = prop_oneof![
            key_char().prop_map(Key::Char),
            named_key().prop_map(Key::Named)
        ];
        (modifiers, key).prop_map(|(modifiers, key)| {
            let modifiers = match key {
                Key::Char(_) => modifiers.without(Modifiers::SHIFT),
                Key::Named(_) => modifiers,
            };
            KeyPress { modifiers, key }
        })
    }

    proptest! {
        /// Printing and parsing a key press gives it back.
        #[test]
        fn prop_key_press_round_trips(key in key_press()) {
            prop_assert_eq!(key.to_string().parse::<KeyPress>(), Ok(key));
        }

        /// Printing and parsing a sequence gives it back.
        #[test]
        fn prop_sequence_round_trips(keys in prop::collection::vec(key_press(), 0..8)) {
            let seq = KeySequence(keys);
            prop_assert_eq!(seq.to_string().parse::<KeySequence>(), Ok(seq));
        }

        /// Whatever parses prints in a form that parses to the same keys.
        #[test]
        fn prop_parse_then_print_is_stable(s in "\\PC{0,24}") {
            if let Ok(seq) = s.parse::<KeySequence>() {
                prop_assert_eq!(seq.to_string().parse::<KeySequence>(), Ok(seq));
            }
        }

        /// Parsing never panics, whatever the input.
        #[test]
        fn prop_parse_never_panics(s in any::<String>()) {
            let _ = s.parse::<KeySequence>();
        }

        /// A parsed key press never holds Shift on a character.
        #[test]
        fn prop_no_shift_on_characters(s in "([CMSs]-){0,4}\\PC") {
            if let Ok(key) = s.parse::<KeyPress>()
                && let Key::Char(_) = key.key
            {
                prop_assert!(!key.modifiers.contains(Modifiers::SHIFT));
            }
        }
    }
}
