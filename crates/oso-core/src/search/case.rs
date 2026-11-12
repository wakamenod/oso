//! A replacement's case after the match it replaces, as Emacs's `case-replace` has it
//! (ADR 0113): only while a search folds case.
//!
//! Must not depend on: `buffer`, `editor` or a log; this sees only two strings.

use std::borrow::Cow;

/// The words of `text`: runs of letters and digits.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

/// `to` in the case of `matched`:
/// - all of `matched`'s letters uppercase, with a word of two letters or more: all of `to`
///   in uppercase;
/// - otherwise, every word of `matched` starting with an uppercase letter: each word of `to`
///   starting with one;
/// - otherwise `to` as it is.
///
/// A script with no case, such as Japanese, is never changed.
pub(crate) fn casified<'a>(matched: &str, to: &'a str) -> Cow<'a, str> {
    let letters = || matched.chars().filter(|c| c.is_alphabetic());
    let has_case = letters().any(|c| c.is_uppercase() || c.is_lowercase());
    if !has_case {
        return Cow::Borrowed(to);
    }
    let all_upper = letters().all(|c| !c.is_lowercase());
    let long_word =
        words(matched).any(|word| word.chars().filter(|c| c.is_alphabetic()).count() >= 2);
    if all_upper && long_word {
        return Cow::Owned(to.to_uppercase());
    }
    let initials = words(matched).all(|word| {
        word.chars()
            .find(|c| c.is_alphabetic())
            .is_none_or(char::is_uppercase)
    });
    if !initials {
        return Cow::Borrowed(to);
    }
    // Each word's first letter in uppercase, the rest as typed.
    let mut out = String::with_capacity(to.len());
    let mut in_word = false;
    for c in to.chars() {
        if c.is_alphanumeric() {
            if in_word {
                out.push(c);
            } else {
                out.extend(c.to_uppercase());
                in_word = true;
            }
        } else {
            out.push(c);
            in_word = false;
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::casified;

    #[test]
    fn the_replacement_takes_the_matchs_case() {
        assert_eq!(casified("foo", "bar"), "bar");
        assert_eq!(casified("FOO", "bar"), "BAR");
        assert_eq!(casified("Foo", "bar"), "Bar");
        assert_eq!(casified("Foo Bar", "baz qux"), "Baz Qux");
        // One letter in uppercase is a capital, not all caps.
        assert_eq!(casified("A", "the"), "The");
        assert_eq!(casified("fOO", "bar"), "bar");
        // Only initials change: the rest stays as typed.
        assert_eq!(casified("Foo", "barBaz"), "BarBaz");
    }

    #[test]
    fn a_match_with_no_case_leaves_it() {
        assert_eq!(casified("日本", "foo"), "foo");
        assert_eq!(casified("123", "foo"), "foo");
    }
}
