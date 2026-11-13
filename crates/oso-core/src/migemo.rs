//! Migemo (ADR 0116): a pattern that finds Japanese text from a romaji string. The string
//! becomes kana by a table of oso's own, and the pattern matches the string itself, its
//! hiragana, katakana and half-width katakana, and its full-width letters, joined as a trie.
//!
//! Must not depend on: any module of this crate, I/O or a log. It uses only
//! `regex::escape`, and compiling the pattern is the caller's.
//!
//! - A string is romaji while it holds only lowercase ASCII letters, `'` and `-`, and at
//!   least one letter. It converts the longest spelling first, so it has one reading: `nn`
//!   is `ん`, as C/Migemo and input methods have it. Within it, `zu` is `ず` or `づ`, and
//!   `ji` and `zi` are `じ` or `ぢ`.
//! - An unfinished end matches every kana it can start: `k` is `か`, `き`, `く`, `け`,
//!   `こ` or `っ`.
//! - A letter that starts no spelling before the end (`struct`) gives no pattern, so the
//!   caller searches for the string as it is.
//! - The pattern grows with the string, at most about 18 bytes a letter, and has no line
//!   break. A string longer than [`MAX_CHARS`] has none.

mod forms;
mod table;
mod trie;

use trie::{Key, Trie};

/// The most characters of a string that has a pattern. Its pattern then compiles in under a
/// millisecond on a key; a longer string is searched for as it is.
pub(crate) const MAX_CHARS: usize = 256;

/// Whether `string` is romaji: only lowercase ASCII letters, `'` and `-`, with at least one
/// letter.
pub(crate) fn is_romaji(string: &str) -> bool {
    string
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b == b'\'' || b == b'-')
        && string.bytes().any(|b| b.is_ascii_lowercase())
}

/// The kana a romaji string gives: the hiragana of what it spells, a character or either of
/// two each, and the hiragana an unfinished end can become, none if it ends finished.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Romaji {
    hiragana: Vec<Key>,
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
        let mut hiragana = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let rest = &string[at..];
            if let Some((spelling, kana)) = longest(rest) {
                match also(spelling) {
                    Some(other) => hiragana.push((one(kana), Some(one(other)))),
                    None => hiragana.extend(kana.chars().map(|c| (c, None))),
                }
                at += spelling.len();
                continue;
            }
            let next = bytes.get(at + 1).copied();
            match bytes[at] {
                b'-' => hiragana.push(('ー', None)),
                b'\'' => {}
                b'n' if next == Some(b'\'') => {
                    hiragana.push(('ん', None));
                    at += 1;
                }
                b'n' if next.is_some_and(|next| next == b'-' || takes_n(next)) => {
                    hiragana.push(('ん', None));
                }
                // Hepburn's `m` for `ん` (`shimbun`). Before `m` it doubles, as input
                // methods have it: `mma` is `っま`.
                b'm' if matches!(next, Some(b'b' | b'p')) => hiragana.push(('ん', None)),
                letter if doubles(letter, &bytes[at + 1..]) => hiragana.push(('っ', None)),
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

/// The second kana `spelling` gives besides its row's, if it gives one.
fn also(spelling: &str) -> Option<&'static str> {
    table::ALSO
        .iter()
        .find(|(also, _)| *also == spelling)
        .map(|(_, kana)| *kana)
}

/// The one character of `kana`, a kana of [`table::ALSO`] or its row's.
fn one(kana: &str) -> char {
    kana.chars().next().unwrap_or_default()
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
    let started = |spelling: &str| spelling.len() > rest.len() && spelling.starts_with(rest);
    let mut ends: Vec<&'static str> = table::ROWS
        .iter()
        .chain(table::ALSO)
        .filter(|(spelling, _)| started(spelling))
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
    let forms: [Form; 3] = [
        |c, out| out.push(c),
        |c, out| out.push(forms::katakana(c)),
        |c, out| forms::push_half_width(forms::katakana(c), out),
    ];
    for form in forms {
        let node = trie.node(keys(&romaji.hiragana, form));
        if romaji.ends.is_empty() {
            node.end();
        }
        for end in &romaji.ends {
            let mut out = String::new();
            end.chars().for_each(|c| form(c, &mut out));
            node.insert(&out);
        }
    }
    let mut out = String::new();
    trie.write(&mut out);
    Some(out)
}

/// A form of the kana: a hiragana's own, pushed onto a string.
type Form = fn(char, &mut String);

/// The keys of `hiragana` in `form`. Either of two kana gives either of two characters at
/// each place their forms differ (`[ｽﾂ]ﾞ`), as both have the same number of characters in
/// each form.
fn keys(hiragana: &[Key], form: Form) -> Vec<Key> {
    let (mut keys, mut first, mut second) = (Vec::new(), String::new(), String::new());
    for &(c, other) in hiragana {
        first.clear();
        form(c, &mut first);
        second.clear();
        if let Some(other) = other {
            form(other, &mut second);
        }
        if second.chars().count() == first.chars().count() {
            let pairs = first.chars().zip(second.chars());
            keys.extend(pairs.map(|(a, b)| (a, (a != b).then_some(b))));
        } else {
            keys.extend(first.chars().map(|a| (a, None)));
        }
    }
    keys
}

/// Every spelling of the table, for tests that build romaji strings from them.
#[cfg(test)]
pub(crate) fn spellings() -> impl Iterator<Item = &'static str> {
    table::ROWS.iter().map(|(spelling, _)| *spelling)
}

#[cfg(test)]
mod tests;
