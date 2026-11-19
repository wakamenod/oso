//! A space's names: a slug is never a device's, a description holds no invisible character, a
//! display name keeps what is drawn and drops what is hidden, and the fallback names a space
//! from its description's ASCII words, else its repository (ADR 0136, ADR 0159).
//!
//! Must not depend on: threads or `app`; plain functions in, text out.

use super::{MAX_DISPLAY, clean_description, clean_display, fallback_names, slug};

#[test]
fn names_are_never_a_device_and_descriptions_hold_no_invisible_characters() {
    assert_eq!(slug("CON"), "con-space");
    assert_eq!(slug("com1"), "com1-space");
    assert_eq!(slug("console"), "console");
    assert_eq!(slug("../.."), "space");
    assert_eq!(clean_description("a\u{202E}b\u{2066}c\td"), "abc d");
    assert_eq!(
        clean_description(&"é".repeat(600)).len(),
        1024,
        "cut at a character's end"
    );
}

#[test]
fn a_display_name_keeps_what_is_drawn_and_drops_what_is_hidden() {
    let family = "👨\u{200D}👩\u{200D}👧";
    assert_eq!(
        clean_display(&format!("  ログイン\t{family}  ")),
        format!("ログイン {family}")
    );
    assert_eq!(
        clean_display("❤\u{FE0F} 1\u{FE0F}\u{20E3}"),
        "❤\u{FE0F} 1\u{FE0F}\u{20E3}"
    );
    let flag = "🏴\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}";
    assert_eq!(clean_display(flag), flag, "a tag sequence on its flag");
    assert_eq!(
        clean_display("a\u{202E}b\u{2066}c\u{200B}d"),
        "abcd",
        "bidi and invisible"
    );
    assert_eq!(clean_display("a\u{200D}b"), "ab", "no joiner after ASCII");
    assert_eq!(
        clean_display("a\u{1b}b\nc\u{85}d"),
        "a b c d",
        "controls part words"
    );
    assert_eq!(
        clean_display("\u{3000}全角\u{3000}空白\u{3000}"),
        "全角 空白"
    );
    let long = clean_display(&family.repeat(20));
    assert!(
        long.len() <= MAX_DISPLAY && long.ends_with('👧'),
        "cut between clusters"
    );
}

#[test]
fn the_fallback_names_from_ascii_words_else_the_repository() {
    let (slug, display) = fallback_names("Fix the login error when SSO expires", "oso");
    assert_eq!(slug, "fix-the-login-error");
    assert_eq!(display, "Fix the login error whe…");
    let (slug, display) = fallback_names("ログイン画面を直す", "oso");
    assert_eq!(
        (slug.as_str(), display.as_str()),
        ("oso", "ログイン画面を直す")
    );
    let (slug, _) = fallback_names("API の v2 対応", "oso");
    assert_eq!(slug, "api-v2");
}

proptest::proptest! {
    #[test]
    fn a_display_name_cleaned_again_stays(
        text in "\\PC{0,40}|[\u{200B}-\u{200F}\u{202A}-\u{202E}\u{0300}-\u{036F}\u{1F1E6}-\u{1F1FF}\u{FE0F}a ]{0,40}"
    ) {
        let once = clean_display(&text);
        proptest::prop_assert_eq!(clean_display(&once), once.clone());
        proptest::prop_assert!(once.len() <= MAX_DISPLAY);
    }
}

#[test]
fn a_display_name_keeps_no_invisible_character_that_could_carry_text() {
    // Tag characters spelling ASCII after a flag, but for the three subdivision flags.
    let tags: String = "rm -rf"
        .chars()
        .map(|c| char::from_u32(0xe0000 + u32::from(c)).unwrap())
        .collect();
    assert_eq!(clean_display(&format!("🏴{tags}\u{e007f}x")), "🏴x");
    // Selectors past the sixteenth, and joiners and selectors after anything but an emoji.
    assert_eq!(clean_display("ログ\u{e0100}イン"), "ログイン");
    assert_eq!(clean_display("ロ\u{200d}グ\u{fe0f}イン"), "ログイン");
    assert_eq!(clean_display("👍\u{fe0f}"), "👍\u{fe0f}");
}

#[test]
fn names_are_compared_with_invisible_characters_dropped_and_ascii_case_aside() {
    use super::skeleton;
    assert_eq!(skeleton("ログ\u{fe0f}イン"), skeleton("ログイン"));
    assert_eq!(skeleton("Fix\u{200b}Login"), "fixlogin");
}
