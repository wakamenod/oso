//! Migemo (ADR 0116): a pattern that finds Japanese text from a romaji string. The string
//! becomes kana by a table of oso's own, and the pattern matches the string itself, its
//! hiragana, katakana and half-width katakana, its full-width letters, and, with a
//! [`Dictionary`], the words of the readings its hiragana start, joined as a trie.
//!
//! Must not depend on: any module of this crate, I/O or a log. It uses only
//! `regex::escape`. [`compile`] holds the pattern's limits, and the caller's builder compiles
//! it, of bytes or of text; reading a dictionary's file is the host's.
//!
//! It also composes a kana and a combining sound mark after it (`か` and U+3099 are `が`), for
//! completion to match file names macOS writes decomposed.
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
//!   break. A string longer than `MAX_CHARS` has none.
//! - The components of one prompt's input share one pattern's words, a [`Budget`] each spends
//!   in turn, and [`alternatives`] lists every string a pattern matches, for a project's
//!   search to tell whether a longer one narrows it (ADR 0116, Q7 and Q8).
//! - [`Table`] is a dictionary the editor holds, told from others by its generation.

use std::sync::Arc;

mod budget;
mod dictionary;
mod forms;
mod table;
mod trie;

pub(crate) use budget::{Budget, Spent};
pub use dictionary::{
    Dictionary, MAX_BYTES, MAX_ENTRIES, MAX_PREFIX_ENTRIES, MAX_READING, MAX_WORD, MAX_WORDS,
    MAX_WORDS_A_READING,
};
#[cfg(test)]
pub(crate) use dictionary::{MAX_PATTERN_WORD_BYTES, word_cost};
use trie::{Key, Trie};

/// The most memory a pattern with a dictionary's words may take once compiled (ADR 0116, Q7):
/// past it, [`compile`] drops the words and keeps the kana.
pub(crate) const WORDS_SIZE_LIMIT: usize = 4 * 1024 * 1024;

/// The deepest a pattern may nest: past it, as past the words' size limit, [`compile`] drops
/// the words.
pub(crate) const NEST_LIMIT: u32 = 32;

/// The most memory the kana's pattern alone may take once compiled, an isearch key's: the
/// largest, of 256 characters, compiles within it on a key.
pub(crate) const KANA_SIZE_LIMIT: usize = 256 * 1024;

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
#[cfg(test)]
pub(crate) fn pattern(string: &str) -> Option<String> {
    Romaji::new(string).map(|romaji| romaji.pattern(string, &[]))
}

impl Romaji {
    /// The pattern for `string`, whose kana these are: the string, its kana in every form,
    /// its full-width letters and each of `words`, a dictionary's.
    pub(crate) fn pattern(&self, string: &str, words: &[&str]) -> String {
        let mut trie = Trie::default();
        for letters in letters(string) {
            trie.insert(&letters);
        }
        for form in FORMS {
            let node = trie.node(keys(&self.hiragana, form));
            if self.ends.is_empty() {
                node.end();
            }
            for end in &self.ends {
                let mut out = String::new();
                end.chars().for_each(|c| form(c, &mut out));
                node.insert(&out);
            }
        }
        // In order, so each goes in after its node's last child, and once.
        let mut words = words.to_vec();
        words.sort_unstable();
        words.dedup();
        for word in words {
            trie.insert(word);
        }
        let mut out = String::new();
        trie.write(&mut out);
        out
    }
}

/// A form of the kana: a hiragana's own, pushed onto a string.
type Form = fn(char, &mut String);

/// The forms of the kana a pattern matches: hiragana, katakana and half-width katakana.
/// [`Romaji::pattern`] and [`alternatives`] take them from here, so they agree.
const FORMS: [Form; 3] = [
    |c, out| out.push(c),
    |c, out| out.push(forms::katakana(c)),
    |c, out| forms::push_half_width(forms::katakana(c), out),
];

/// The letters a pattern matches besides the kana: the string, and its full-width letters.
fn letters(string: &str) -> impl Iterator<Item = String> {
    [
        string.to_owned(),
        string.chars().map(forms::full_width).collect(),
    ]
    .into_iter()
}

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

/// Migemo's pattern for `string`, compiled by `build` from a pattern's text within a size
/// limit (ADR 0116): with `dictionary`'s words within [`WORDS_SIZE_LIMIT`], or, if there are
/// none or that does not compile, the kana alone within [`KANA_SIZE_LIMIT`]. `None` if the
/// string gives no kana (see [`Romaji::new`]) or not even the kana's pattern compiles, and
/// the caller matches the string as it is. `build` sets the case and [`NEST_LIMIT`].
pub(crate) fn compile<R>(
    string: &str,
    dictionary: Option<&Dictionary>,
    words_limit: usize,
    build: impl FnMut(&str, usize) -> Option<R>,
) -> Option<R> {
    let mut budget = Budget::PATTERN;
    compile_spending(string, dictionary, words_limit, &mut budget, build).compiled
}

/// A pattern [`compile_with_budget`] compiled, if it compiled, the words that went into it
/// and what they spent, which were tried even if they did not compile.
pub(crate) struct Compiled<'d, R> {
    pub(crate) compiled: Option<R>,
    pub(crate) words: Vec<&'d str>,
    pub(crate) spent: Spent,
}

/// [`compile`], with only the words `budget` leaves, which it spends: what one input's
/// components compile on the UI thread (ADR 0116, Q7).
pub(crate) fn compile_with_budget<'d, R>(
    string: &str,
    dictionary: Option<&'d Dictionary>,
    budget: &mut Budget,
    build: impl FnMut(&str, usize) -> Option<R>,
) -> Compiled<'d, R> {
    compile_spending(string, dictionary, WORDS_SIZE_LIMIT, budget, build)
}

/// [`compile_with_budget`], the words within `words_limit`.
fn compile_spending<'d, R>(
    string: &str,
    dictionary: Option<&'d Dictionary>,
    words_limit: usize,
    budget: &mut Budget,
    mut build: impl FnMut(&str, usize) -> Option<R>,
) -> Compiled<'d, R> {
    let Some(romaji) = Romaji::new(string) else {
        return Compiled {
            compiled: None,
            words: Vec::new(),
            spent: Spent::default(),
        };
    };
    let (words, spent) = dictionary.map_or_else(Default::default, |dictionary| {
        dictionary.pattern_words_with_budget(&romaji, *budget)
    });
    budget.spend(spent.used);
    if !words.is_empty()
        && let Some(compiled) = build(&romaji.pattern(string, &words), words_limit)
    {
        return Compiled {
            compiled: Some(compiled),
            words,
            spent,
        };
    }
    Compiled {
        compiled: build(&romaji.pattern(string, &[]), KANA_SIZE_LIMIT),
        words: Vec::new(),
        spent,
    }
}

/// Every string migemo's pattern for `string` with `words` matches, as [`Romaji::pattern`]
/// writes it, `None` if it gives no kana, if they would be more than `count` besides the
/// words, as either of two kana and each unfinished end multiply them, or if they would take
/// more than `bytes`, the words' included. What it builds stays within those.
pub(crate) fn alternatives(
    string: &str,
    words: &[&str],
    (count, bytes): (usize, usize),
) -> Option<Vec<Box<str>>> {
    let romaji = Romaji::new(string)?;
    let forms = FORMS.map(|form| (form, keys(&romaji.hiragana, form)));
    let mut total = 0usize;
    for (_, keys) in &forms {
        let pairs = keys.iter().filter(|(_, other)| other.is_some()).count();
        let each = 1usize.checked_shl(u32::try_from(pairs).ok()?)?;
        total = total.checked_add(each.checked_mul(romaji.ends.len().max(1))?)?;
    }
    if total > count {
        return None;
    }
    let mut size: usize = words.iter().map(|word| word.len()).sum();
    let mut out: Vec<Box<str>> = Vec::new();
    let mut push = |string: String, out: &mut Vec<Box<str>>| {
        size += string.len();
        out.push(string.into());
        size <= bytes
    };
    for letters in letters(string) {
        if !push(letters, &mut out) {
            return None;
        }
    }
    for (form, keys) in forms {
        let mut stems = vec![String::new()];
        for &(c, other) in &keys {
            match other {
                None => stems.iter_mut().for_each(|stem| stem.push(c)),
                Some(other) => {
                    let mut with = stems.clone();
                    stems.iter_mut().for_each(|stem| stem.push(c));
                    with.iter_mut().for_each(|stem| stem.push(other));
                    stems.append(&mut with);
                }
            }
        }
        for stem in stems {
            if romaji.ends.is_empty() {
                if !push(stem, &mut out) {
                    return None;
                }
                continue;
            }
            for end in &romaji.ends {
                let mut alternative = stem.clone();
                end.chars().for_each(|c| form(c, &mut alternative));
                if !push(alternative, &mut out) {
                    return None;
                }
            }
        }
    }
    out.extend(words.iter().map(|&word| word.into()));
    Some(out)
}

/// The combining sound marks, U+3099 and U+309A, which a kana before them may compose with.
pub(crate) const SOUND_MARKS: [char; 2] = ['\u{3099}', '\u{309A}'];

/// A dictionary the editor holds, and the generation that tells it from every other it held.
/// A compiled query keeps only the generation, so it never keeps a table alive, and only a
/// job holds the table, while it runs.
#[derive(Clone)]
pub(crate) struct Table {
    pub(crate) generation: u64,
    pub(crate) dictionary: Arc<Dictionary>,
}

/// The generation of `table`, `None` for none: what a migemo component compiled with it keeps.
pub(crate) fn generation(table: Option<&Table>) -> Option<u64> {
    table.map(|table| table.generation)
}

/// Whether `string` is romaji that gives kana, and so has migemo's pattern.
pub(crate) fn gives_kana(string: &str) -> bool {
    Romaji::new(string).is_some()
}

/// The kana `base` and `mark`, a combining sound mark, compose to: `か` and U+3099 are `が`,
/// as Unicode's canonical composition has it. `None` where they do not compose.
pub(crate) fn composed(base: char, mark: char) -> Option<char> {
    forms::composed(base, mark)
}

/// Every spelling of the table, for tests that build romaji strings from them.
#[cfg(test)]
pub(crate) fn spellings() -> impl Iterator<Item = &'static str> {
    table::ROWS.iter().map(|(spelling, _)| *spelling)
}

#[cfg(test)]
mod tests;
