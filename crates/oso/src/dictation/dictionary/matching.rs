//! Which words TypeWhisper may take for the same (ADR 0175, step 4). It ignores case and, for
//! terms, accents, and takes characters with the same meaning in Unicode for the same.
//!
//! Must not depend on: anything of the crate; this compares two texts.
//!
//! - [`Key::like`] errs to yes, so that oso leaves alone what it cannot tell from its own.
//! - [`is_plain`] says which words oso adds: those it can tell apart.

/// A word as it is compared, folded once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    chars: Vec<char>,
    /// No mark was cut from it.
    whole: bool,
}

impl Key {
    /// `text`'s key.
    pub fn of(text: &str) -> Self {
        let mut whole = true;
        let mut chars = Vec::with_capacity(text.len());
        for c in text.trim().chars() {
            if is_combining(c) {
                whole = false;
                continue;
            }
            match ligature(c) {
                Some(letters) => chars.extend(letters.chars()),
                None => chars.extend(c.to_lowercase().filter(|c| !is_combining(*c))),
            }
        }
        Self { chars, whole }
    }

    /// Whether TypeWhisper may take the two for the same.
    pub fn like(&self, other: &Self) -> bool {
        let whole = self.whole && other.whole;
        self.chars.len() == other.chars.len()
            && (self.chars.iter().zip(&other.chars)).all(|(&a, &b)| alike(a, b, whole))
    }
}

fn alike(a: char, b: char, whole: bool) -> bool {
    if a == b {
        return true;
    }
    match (a.is_ascii(), b.is_ascii()) {
        (true, true) => false,
        (true, false) => is_accented(b),
        (false, true) => is_accented(a),
        // Two accents of a letter, a character that stands for another, or where a mark was cut.
        (false, false) => {
            !whole || is_accented(a) && is_accented(b) || is_compatible(a) || is_compatible(b)
        }
    }
}

/// Whether oso can tell `text` apart from every other word by [`Key::like`]: no mark of its own,
/// no character that stands for others.
pub fn is_plain(text: &str) -> bool {
    !text.chars().any(|c| {
        is_combining(c)
            || is_compatible(c)
            || ligature(c).is_some()
            // Hangul, whose syllables and their parts are the same.
            || matches!(c, '\u{1100}'..='\u{11ff}' | '\u{a960}'..='\u{a97f}' | '\u{ac00}'..='\u{d7ff}')
            || matches!(c, 'İ' | 'ı' | 'ſ')
    })
}

/// The letters `c` is taken for when case is ignored.
fn ligature(c: char) -> Option<&'static str> {
    Some(match c {
        'ß' | 'ẞ' => "ss",
        'ﬀ' => "ff",
        'ﬁ' => "fi",
        'ﬂ' => "fl",
        'ﬃ' => "ffi",
        'ﬄ' => "ffl",
        'ﬅ' | 'ﬆ' => "st",
        'ŉ' => "n",
        'ſ' => "s",
        _ => return None,
    })
}

/// Whether `c` may be a Latin, Greek or Cyrillic letter that has an accent.
fn is_accented(c: char) -> bool {
    matches!(c, '\u{80}'..='\u{52f}' | '\u{1e00}'..='\u{1fff}' | '\u{2126}' | '\u{212a}' | '\u{212b}')
}

/// Whether `c` is an ideograph or a sign that stands for another.
fn is_compatible(c: char) -> bool {
    matches!(c, '\u{2000}'..='\u{2001}' | '\u{2329}' | '\u{232a}' | '\u{f900}'..='\u{faff}' | '\u{2f800}'..='\u{2fa1f}')
}

/// Whether `c` is a mark that joins the letter before it, among the accents and the kana's.
fn is_combining(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}'
        | '\u{20d0}'..='\u{20ff}' | '\u{3099}' | '\u{309a}' | '\u{fe20}'..='\u{fe2f}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn like(a: &str, b: &str) -> bool {
        Key::of(a).like(&Key::of(b))
    }

    #[test]
    fn case_accents_and_marks_make_no_difference() {
        for (a, b) in [
            ("React", "react"),
            ("ETE", "été"),
            ("Café", "cafe\u{301}"),
            ("がぞう", "か\u{3099}ぞう"),
            ("神社", "\u{fa19}社"),
            ("strasse", "Straße"),
            ("fish", "ﬁsh"),
            (" oso ", "oso"),
        ] {
            assert!(like(a, b) && like(b, a), "{a} and {b}");
        }
    }

    #[test]
    fn other_words_are_told_apart() {
        for (a, b) in [
            ("AI", "東京"),
            ("東京", "京都"),
            ("機能", "昨日"),
            ("oso", "ost"),
            ("かぞう", "がぞう"),
            ("a", "aa"),
        ] {
            assert!(!like(a, b), "{a} and {b}");
        }
    }

    #[test]
    fn oso_adds_only_words_it_can_tell_apart() {
        assert!(is_plain("がぞう React 昨日 été"));
        for text in ["か\u{3099}", "\u{fa19}", "ß", "ﬁ", "한글"] {
            assert!(!is_plain(text), "{text}");
        }
    }
}
