//! Which learned pairs are promoted to TypeWhisper's dictionary (ADR 0175, step 4), and as what.
//!
//! Must not depend on: the list, the calls or the app; this judges a pair by the record and by
//! what TypeWhisper holds.
//!
//! - **A pair is promoted** once it stood [`PROMOTE_STOOD`] times, over [`PROMOTE_DAYS`] days
//!   or more, and more often than it was undone. One learned only from text typed plain, which
//!   the person kept from Claude, never is: TypeWhisper's own settings may send its dictionary
//!   on (the author's choice on 2026-11-24).
//! - One whose `wrong` is kana and whose `right` is ASCII becomes a term; any other a
//!   correction.

use super::matching::{Key, is_plain};
use super::{Dictionary, Kind};
use crate::dictation::learning::{Pair, Record};

/// How often a pair stood before it is promoted.
pub const PROMOTE_STOOD: u32 = 3;
/// Over how many days, at least.
pub const PROMOTE_DAYS: usize = 2;
/// The most characters of either side of an entry.
pub const MAX_CHARS: usize = 40;

/// Whether `pair` was seen often enough to be promoted.
pub fn ready(pair: &Pair) -> bool {
    pair.sent_stood() > 0
        && pair.stood() >= PROMOTE_STOOD
        && pair.days.len() >= PROMOTE_DAYS
        && pair.stood() > pair.undone
}

/// What `pair` would be in TypeWhisper's dictionary.
pub fn kind_of(pair: &Pair) -> Kind {
    let kana = pair.wrong.chars().all(is_kana);
    let ascii = pair.right.is_ascii() && pair.right.chars().any(|c| c.is_ascii_alphabetic());
    if kana && ascii {
        Kind::Term
    } else {
        Kind::Correction
    }
}

/// Why `pair`, seen often enough, is not promoted, if its own text or `record` says so.
pub fn unfit(pair: &Pair, record: &Record) -> Option<&'static str> {
    let sides = [pair.wrong.as_str(), pair.right.as_str()];
    let chars = pair.wrong.chars().count();
    let of_few = pair.wrong.chars().all(is_hiragana) || pair.wrong.is_ascii();
    if sides
        .iter()
        .any(|side| side.trim() != *side || side.is_empty())
    {
        return Some("a side is empty, or starts or ends with a space");
    }
    if sides
        .iter()
        .any(|side| side.chars().count() > MAX_CHARS || !is_plain(side))
    {
        return Some("a side is too long, or holds a character other ways to write it");
    }
    if chars < 2 || (chars < 3 && of_few) || !pair.wrong.chars().any(char::is_alphabetic) {
        return Some("its wrong is too short");
    }
    let (wrong, right) = (pair.wrong.to_lowercase(), pair.right.to_lowercase());
    if wrong == right {
        return Some("only its case differs");
    }
    if right.contains(&wrong) {
        return Some("its right holds its wrong");
    }
    let others = (record.pairs.iter())
        .filter(|other| other.wrong == pair.wrong && other.right != pair.right)
        .fold(0_u32, |sum, other| sum.saturating_add(other.stood()));
    if others >= pair.stood() {
        return Some("its wrong became other words as often");
    }
    if taken_back(pair, record, 0) {
        return Some("you took it back");
    }
    None
}

/// Whether `record` says the person took `pair` back since the day `since`: it was undone as
/// often as it stood, or they rewrote its `right` to its `wrong` themselves.
pub fn taken_back(pair: &Pair, record: &Record, since: u32) -> bool {
    (pair.undone > 0 && pair.undone >= pair.stood())
        || record.pairs.iter().any(|back| {
            back.wrong == pair.right
                && back.right == pair.wrong
                && back.edits.saturating_add(back.plain) > 0
                && back.days.last().is_some_and(|day| *day >= since)
        })
}

/// Why `pair`, as `kind`, may not go beside what `dictionary` holds.
pub fn clashes(pair: (&str, &str), kind: Kind, dictionary: &Dictionary) -> Option<&'static str> {
    let key = Key::of(kind.known_by(pair.0, pair.1));
    match kind {
        Kind::Term => (dictionary.terms.iter())
            .any(|term| term.key.like(&key))
            .then_some("TypeWhisper has the term"),
        Kind::Correction => {
            let (wrong, right) = (pair.0.to_lowercase(), pair.1.to_lowercase());
            dictionary.corrections.iter().find_map(|other| {
                let (original, replacement) = &other.lower;
                if other.key.like(&key) {
                    Some("TypeWhisper has a correction of its wrong")
                } else if !original.is_empty() && right.contains(original)
                    || replacement.contains(&wrong)
                {
                    Some("another correction would change it, or it another")
                } else {
                    None
                }
            })
        }
    }
}

fn is_hiragana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{3096}' | '\u{309d}' | '\u{309e}' | 'ー')
}

fn is_kana(c: char) -> bool {
    is_hiragana(c) || matches!(c, '\u{30a1}'..='\u{30fe}' | '\u{ff66}'..='\u{ff9f}')
}

#[cfg(test)]
mod tests;
