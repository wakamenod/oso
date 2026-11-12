//! Migemo (ADR 0116): a pattern that finds Japanese text from a romaji string. The string
//! becomes kana by a table of oso's own, and the pattern matches the string itself, its
//! hiragana, katakana and half-width katakana, and its full-width letters, joined as a trie.
//!
//! Must not depend on: any module of this crate, I/O or a log. It uses only
//! `regex::escape`, and compiling the pattern is the caller's.
//!
//! - A string is romaji while it holds only lowercase ASCII letters, `'` and `-`, and at
//!   least one letter. It converts the longest spelling first, so it has one reading: `nn`
//!   is `ん`, as C/Migemo and input methods have it.
//! - An unfinished end matches every kana it can start: `k` is `か`, `き`, `く`, `け`,
//!   `こ` or `っ`.
//! - A letter that starts no spelling before the end (`struct`) gives no pattern, so the
//!   caller searches for the string as it is.
//! - The pattern grows with the string, at most about 16 bytes a letter, and has no line
//!   break. A string longer than [`MAX_CHARS`] has none.

mod forms;
mod table;
mod trie;

use trie::Trie;

/// The most characters of a string that has a pattern. Its pattern then compiles in about
/// a millisecond, as a key's regexp does; a longer string is searched for as it is.
pub(crate) const MAX_CHARS: usize = 256;

/// Whether `string` is romaji: only lowercase ASCII letters, `'` and `-`, with at least one
/// letter.
pub(crate) fn is_romaji(string: &str) -> bool {
    string
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b == b'\'' || b == b'-')
        && string.bytes().any(|b| b.is_ascii_lowercase())
}

/// The kana a romaji string gives: the hiragana of what it spells, and the hiragana an
/// unfinished end can become, none if it ends finished.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Romaji {
    hiragana: String,
    ends: Vec<&'static str>,
}

impl Romaji {
    /// The kana `string` gives, `None` if it is not romaji, is longer than [`MAX_CHARS`], or
    /// has a letter that starts no spelling before its end.
    pub(crate) fn new(string: &str) -> Option<Self> {
        if !is_romaji(string) || string.len() > MAX_CHARS {
            return None;
        }
        let bytes = string.as_bytes();
        let mut hiragana = String::new();
        let mut at = 0;
        while at < bytes.len() {
            let rest = &string[at..];
            if let Some((spelling, kana)) = longest(rest) {
                hiragana.push_str(kana);
                at += spelling.len();
                continue;
            }
            let next = bytes.get(at + 1).copied();
            match bytes[at] {
                b'-' => hiragana.push('ー'),
                b'\'' => {}
                b'n' if next == Some(b'\'') => {
                    hiragana.push('ん');
                    at += 1;
                }
                b'n' if next.is_some_and(|next| next == b'-' || takes_n(next)) => {
                    hiragana.push('ん');
                }
                letter if doubles(letter, &bytes[at + 1..]) => hiragana.push('っ'),
                _ => {
                    let ends = ends(rest);
                    return (!ends.is_empty()).then_some(Self { hiragana, ends });
                }
            }
            at += 1;
        }
        Some(Self {
            hiragana,
            ends: Vec::new(),
        })
    }
}

/// The spelling `rest` starts with, and its hiragana: as no spelling starts another, there
/// is at most one.
fn longest(rest: &str) -> Option<(&'static str, &'static str)> {
    table::ROWS
        .iter()
        .find(|(spelling, _)| rest.starts_with(spelling))
        .copied()
}

/// Whether an `n` before `letter` is `ん`: before a consonant other than `n` and `y`.
fn takes_n(letter: u8) -> bool {
    letter.is_ascii_lowercase() && !b"aeiouny".contains(&letter)
}

/// Whether `letter`, followed by `after`, is a doubled consonant's `っ`: `kka`, and `tch`
/// as input methods take it.
fn doubles(letter: u8, after: &[u8]) -> bool {
    let consonant = letter.is_ascii_lowercase() && !b"aeioun".contains(&letter);
    consonant
        && match after {
            [next, ..] if *next == letter => true,
            [b'c'] | [b'c', b'h', ..] => letter == b't',
            _ => false,
        }
}

/// The hiragana an unfinished end can become: every spelling it starts, and `っ` after a
/// consonant that doubles.
fn ends(rest: &str) -> Vec<&'static str> {
    let mut ends: Vec<&'static str> = table::ROWS
        .iter()
        .filter(|(spelling, _)| spelling.len() > rest.len() && spelling.starts_with(rest))
        .map(|(_, kana)| *kana)
        .collect();
    if let [letter] = rest.as_bytes()
        && doubles(*letter, &[*letter])
    {
        ends.push("っ");
    }
    ends.sort_unstable();
    ends.dedup();
    ends
}

/// The pattern for `string`, a regexp's text that matches the string, its kana in every
/// form and its full-width letters, `None` if it gives no kana (see [`Romaji::new`]). It
/// holds no flag: the caller compiles it with the case it wants.
pub(crate) fn pattern(string: &str) -> Option<String> {
    let romaji = Romaji::new(string)?;
    let mut trie = Trie::default();
    trie.insert(string);
    trie.insert(&string.chars().map(forms::full_width).collect::<String>());
    let katakana = |kana: &str| kana.chars().map(forms::katakana).collect::<String>();
    let half_width = |kana: &str| {
        let mut out = String::new();
        for c in kana.chars() {
            forms::push_half_width(forms::katakana(c), &mut out);
        }
        out
    };
    let forms: [&dyn Fn(&str) -> String; 3] = [&str::to_owned, &katakana, &half_width];
    for form in forms {
        let node = trie.node(&form(&romaji.hiragana));
        if romaji.ends.is_empty() {
            node.end();
        }
        for end in &romaji.ends {
            node.insert(&form(end));
        }
    }
    let mut out = String::new();
    trie.write(&mut out);
    Some(out)
}

/// Every spelling of the table, for tests that build romaji strings from them.
#[cfg(test)]
pub(crate) fn spellings() -> impl Iterator<Item = &'static str> {
    table::ROWS.iter().map(|(spelling, _)| *spelling)
}

#[cfg(test)]
mod tests;
