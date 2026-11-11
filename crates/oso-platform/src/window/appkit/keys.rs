//! What an AppKit key event says, as `window`'s types: the named keys by their virtual key
//! code and the modifiers held, and where a key or the input method's text goes (ADR 0070).
//! Plain functions, tested without a window.
//!
//! Must not depend on: the view or the run loop; nothing here calls AppKit.

use objc2_app_kit::NSEventModifierFlags;

use crate::window::{Held, InputSource, SystemNamed};

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

/// Where a key goes, decided before the input method sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    /// To oso as a key press.
    Oso,
    /// Commit the text being composed, then to oso.
    CommitThenOso,
    /// Drop the text being composed; the key goes nowhere.
    Discard,
    /// Drop the text being composed, then to oso.
    DiscardThenOso,
    /// To the input method, which may give it back.
    InputMethod,
}

/// Where a key goes with `held`, whether text is `marked`, whether oso has the input method
/// `allowed`, and whether the key is `g` on the layout.
///
/// - The input method off: the key is oso's, and text marked meanwhile is dropped, so none
///   lives on in a buffer where it is off.
/// - Text marked: Option or Command commits it and runs, `C-g` alone drops it, and any other
///   key goes to the input method.
/// - Nothing marked: Control, Command or Option makes the key oso's.
pub(super) fn route(held: Held, marked: bool, allowed: bool, is_g: bool) -> Route {
    let option_or_command = held.contains(Held::ALT) || held.contains(Held::LOGO);
    match (allowed, marked) {
        (false, true) => Route::DiscardThenOso,
        (false, false) => Route::Oso,
        (true, true) if option_or_command => Route::CommitThenOso,
        (true, true) if held == Held::CONTROL && is_g => Route::Discard,
        (true, true) => Route::InputMethod,
        (true, false) if option_or_command || held.contains(Held::CONTROL) => Route::Oso,
        (true, false) => Route::InputMethod,
    }
}

/// Whether the view takes text or marked text the input method sends with no key of oso's in
/// hand: only while the input method is on, and not just after oso dropped a composition, as
/// a reply to that may come late. A key clears that.
pub(super) fn takes_text(allowed: bool, muted: bool) -> bool {
    allowed && !muted
}

/// Whether text the input method inserts while handling a key is that key's own, which goes to
/// oso as the key: one character, the same the key types. A full-width space or letter from a
/// Japanese input mode is not, and is a commit.
pub(super) fn is_keys_own(text: &str, characters: Option<&str>) -> bool {
    characters == Some(text) && text.chars().nth(1).is_none()
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

/// Whether the key oso takes is the one Shift alone would type, and not AppKit's
/// `characters`. With Control or Command held, `characters` leaves Shift out of a punctuation
/// key on some layouts and input sources (`,` for Command-Shift-`,`), or gives a control
/// character, where Shift belongs in the character (ADR 0033): `M-<`, `C->`.
pub(super) fn shift_only(held: Held) -> bool {
    held.contains(Held::CONTROL) || held.contains(Held::LOGO)
}

/// The key oso takes with Control or Command held ([`shift_only`]): what Shift alone types
/// (`shifted`), if that is a printable ASCII character. Otherwise, the layout's `other`
/// reading if that one is: the Command layer with Command held, which is Latin on Russian,
/// Greek or Hebrew layouts (`x`, where Shift alone types `ч`), and AppKit's `characters`
/// with Control, which gives `2` on AZERTY's digit row (where Shift alone types `é`).
/// Otherwise what Shift alone types. `other` is read only when it is needed.
pub(super) fn chosen_key<'a>(
    shifted: Option<&'a str>,
    other: impl FnOnce() -> Option<&'a str>,
) -> Option<&'a str> {
    let ascii = |text: &str| {
        let mut chars = text.chars();
        matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_graphic())
    };
    if shifted.is_some_and(ascii) {
        return shifted;
    }
    other().filter(|text| ascii(text)).or(shifted)
}

/// The kind of input source the Text Input Sources describe (ADR 0106): a Japanese input
/// mode is one that cannot type ASCII and whose ID names Japanese, as Kotoeri's
/// `…Kotoeri.RomajiTyping.Japanese`, Google's `…inputmethod.Japanese.base` and other input
/// methods' Japanese modes do. Their modes for Latin letters (`….Roman`), keyboard layouts and
/// other languages' input methods are `Other`.
pub(super) fn input_source(id: &str, ascii_capable: bool) -> InputSource {
    if !ascii_capable && id.contains("Japanese") {
        InputSource::Japanese
    } else {
        InputSource::Other
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Route, chosen_key, held, input_source, is_keys_own, named, route, shift_only, takes_text,
    };
    use crate::window::InputSource;
    use crate::window::{Held, SystemNamed};
    use objc2_app_kit::NSEventModifierFlags as F;

    #[test]
    fn with_control_or_command_the_key_is_what_shift_alone_types() {
        for held_keys in [
            Held::CONTROL,
            Held::LOGO,
            Held::CONTROL | Held::LOGO,
            Held::CONTROL | Held::SHIFT,
            Held::LOGO | Held::SHIFT,
            Held::CONTROL | Held::LOGO | Held::SHIFT,
            Held::CONTROL | Held::ALT,
        ] {
            assert!(shift_only(held_keys), "{held_keys:?}");
        }
        for held_keys in [Held::NONE, Held::SHIFT, Held::ALT, Held::ALT | Held::SHIFT] {
            assert!(!shift_only(held_keys), "{held_keys:?}");
        }
    }

    #[test]
    fn the_key_is_ascii_from_shift_alone_or_from_the_layouts_other_reading() {
        let never = || -> Option<&str> { panic!("read only when needed") };
        // US: Command-Shift-`,` and Control-Shift-`.`, where `characters` left Shift out.
        assert_eq!(chosen_key(Some("<"), never), Some("<"));
        assert_eq!(chosen_key(Some(">"), never), Some(">"));
        // Russian: Command's layer is Latin, with Shift too.
        assert_eq!(chosen_key(Some("ч"), || Some("x")), Some("x"), "M-x");
        assert_eq!(chosen_key(Some("Б"), || Some("<")), Some("<"), "M-<");
        // AZERTY's digit row with Control: `characters` gives the digit.
        assert_eq!(chosen_key(Some("é"), || Some("2")), Some("2"), "C-2");
        // Russian with Control: `characters` is a control character, so the letter stays.
        assert_eq!(chosen_key(Some("ч"), || Some("\u{18}")), Some("ч"));
        assert_eq!(chosen_key(Some("ч"), || None), Some("ч"));
        assert_eq!(
            chosen_key(Some("ч"), || Some("xy")),
            Some("ч"),
            "one character"
        );
        assert_eq!(chosen_key(None, || Some("x")), Some("x"));
        assert_eq!(chosen_key(None, || None), None);
    }

    #[test]
    fn only_a_japanese_input_mode_counts_as_japanese() {
        for id in [
            "com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese",
            "com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese.Katakana",
            "com.apple.inputmethod.Kotoeri.KanaTyping.Japanese",
            "com.google.inputmethod.Japanese.base",
            "com.justsystems.inputmethod.atok34.Japanese",
        ] {
            assert_eq!(input_source(id, false), InputSource::Japanese, "{id}");
            assert_eq!(
                input_source(id, true),
                InputSource::Other,
                "{id} typing ASCII"
            );
        }
        // Latin modes and layouts type ASCII; other languages' input methods are not Japanese.
        for (id, ascii) in [
            ("com.apple.keylayout.ABC", true),
            ("com.apple.inputmethod.Kotoeri.RomajiTyping.Roman", true),
            ("com.google.inputmethod.Japanese.Roman", true),
            ("com.apple.inputmethod.SCIM.ITABC", false),
            ("com.apple.inputmethod.Korean.2SetKorean", false),
            ("", false),
        ] {
            assert_eq!(input_source(id, ascii), InputSource::Other, "{id}");
        }
    }

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
    fn keys_go_to_oso_unless_the_input_method_is_on_and_wants_them() {
        let (none, control, option) = (Held::NONE, Held::CONTROL, Held::ALT);
        assert_eq!(route(none, false, true, false), Route::InputMethod);
        assert_eq!(route(control, false, true, false), Route::Oso);
        assert_eq!(
            route(option, false, true, false),
            Route::Oso,
            "Option is Super"
        );
        assert_eq!(route(Held::LOGO, false, true, false), Route::Oso);
        assert_eq!(
            route(none, false, false, false),
            Route::Oso,
            "the IME is off"
        );
    }

    #[test]
    fn text_being_composed_is_committed_dropped_or_kept() {
        let (none, control) = (Held::NONE, Held::CONTROL);
        assert_eq!(route(Held::ALT, true, true, false), Route::CommitThenOso);
        assert_eq!(route(Held::LOGO, true, true, false), Route::CommitThenOso);
        assert_eq!(route(control, true, true, true), Route::Discard, "C-g");
        let control_shift = control | Held::SHIFT;
        assert_eq!(
            route(control_shift, true, true, true),
            Route::InputMethod,
            "C-S-g"
        );
        assert_eq!(
            route(control, true, true, false),
            Route::InputMethod,
            "other C-"
        );
        assert_eq!(route(none, true, true, false), Route::InputMethod);
        assert_eq!(
            route(none, true, false, false),
            Route::DiscardThenOso,
            "IME off"
        );
        assert_eq!(route(control, true, false, true), Route::DiscardThenOso);
    }

    #[test]
    fn text_with_no_key_is_taken_only_while_the_ime_is_on_and_not_just_dropped() {
        assert!(takes_text(true, false));
        assert!(
            !takes_text(false, false),
            "the IME is off, as in a secret buffer"
        );
        assert!(!takes_text(true, true), "a late reply to a drop");
        assert!(!takes_text(false, true));
    }

    #[test]
    fn only_the_keys_own_character_goes_to_oso_as_the_key() {
        assert!(is_keys_own("a", Some("a")));
        assert!(is_keys_own(" ", Some(" ")));
        assert!(
            !is_keys_own("\u{3000}", Some(" ")),
            "a full-width space is a commit"
        );
        assert!(
            !is_keys_own("ａ", Some("a")),
            "a full-width letter is a commit"
        );
        assert!(
            !is_keys_own("ab", Some("ab")),
            "several characters are text"
        );
        assert!(!is_keys_own("a", None));
    }
}
