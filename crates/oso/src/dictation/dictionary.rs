//! TypeWhisper's dictionary as oso adds to it (ADR 0175, step 4): what TypeWhisper gives of it,
//! and what oso's entries are there.
//!
//! Must not depend on: `app`, `sessions` or the editor, or how a call is made.
//!
//! - `matching` says which words TypeWhisper may take for the same.
//! - `rules` says which learned pairs are promoted, and as what.
//! - `list` is what oso keeps of the entries it added.
//! - `sync` makes one pass over the dictionary.

use std::collections::HashSet;

use serde_json::Value;

pub mod list;
pub mod matching;
pub mod rules;
pub mod sync;

use list::Entry;
use matching::Key;

/// The most entries of each half of TypeWhisper's dictionary oso looks through: with more, it
/// is taken as not read.
const MAX_THERE: usize = 10_000;
/// The most bytes of a word there.
const MAX_WORD: usize = 1024;

/// What an entry is in TypeWhisper's dictionary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The `right` alone, which the recogniser is hinted with.
    Term,
    /// The `wrong` replaced by the `right`, matching case.
    Correction,
}

impl Kind {
    /// As the list and the log write it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Term => "term",
            Self::Correction => "correction",
        }
    }

    /// The kind called `name`.
    pub fn of(name: &str) -> Option<Self> {
        [Self::Term, Self::Correction]
            .into_iter()
            .find(|kind| kind.name() == name)
    }

    /// What TypeWhisper knows an entry of this kind by, of its `wrong` and `right`.
    pub fn known_by<'a>(self, wrong: &'a str, right: &'a str) -> &'a str {
        match self {
            Self::Term => right,
            Self::Correction => wrong,
        }
    }
}

/// A term as TypeWhisper gives it.
#[derive(Clone, Debug)]
pub struct Term {
    text: String,
    key: Key,
    /// The person gave it a similarity of its own.
    tuned: bool,
}

/// A correction as TypeWhisper gives it.
#[derive(Clone, Debug)]
pub struct Correction {
    original: String,
    replacement: String,
    matches_case: bool,
    key: Key,
    /// Both sides in lower case.
    lower: (String, String),
}

/// TypeWhisper's dictionary as its API gives it: what is enabled.
#[derive(Clone, Debug, Default)]
pub struct Dictionary {
    pub terms: Vec<Term>,
    pub corrections: Vec<Correction>,
}

/// The terms in TypeWhisper's answer, whole; nothing if it is not one oso reads.
pub fn terms(body: &[u8]) -> Option<Vec<Term>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let tuned: HashSet<&str> = (value.get("term_entries"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| {
            (entry.get("ctc_min_similarity")).is_some_and(|similarity| !similarity.is_null())
        })
        .filter_map(|entry| entry.get("term")?.as_str())
        .collect();
    let listed = value.get("terms")?.as_array()?;
    if listed.len() > MAX_THERE {
        return None;
    }
    (listed.iter())
        .map(|term| {
            let text = word(term)?;
            Some(Term {
                key: Key::of(&text),
                tuned: tuned.contains(text.as_str()),
                text,
            })
        })
        .collect()
}

/// The corrections in TypeWhisper's answer, whole; nothing if it is not one oso reads.
pub fn corrections(body: &[u8]) -> Option<Vec<Correction>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let listed = value.get("corrections")?.as_array()?;
    if listed.len() > MAX_THERE {
        return None;
    }
    (listed.iter())
        .map(|entry| {
            let original = word(entry.get("original")?)?;
            let replacement = word(entry.get("replacement")?)?;
            Some(Correction {
                matches_case: entry.get("caseSensitive")?.as_bool()?,
                key: Key::of(&original),
                lower: (original.to_lowercase(), replacement.to_lowercase()),
                original,
                replacement,
            })
        })
        .collect()
}

fn word(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    (text.len() <= MAX_WORD).then(|| text.to_owned())
}

/// How TypeWhisper holds an entry of oso's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// Exactly as oso added it, and nothing else that a removal could take instead.
    Exactly,
    /// Changed, or with another beside it that TypeWhisper may take for the same.
    Otherwise,
    /// Not at all, as far as it says.
    No,
}

impl Dictionary {
    /// How it holds `entry`.
    pub fn holds(&self, entry: &Entry) -> Held {
        let key = Key::of(entry.known_by());
        let mut like = match entry.kind {
            Kind::Term => (self.terms.iter())
                .filter(|term| term.key.like(&key))
                .map(|term| term.text == entry.right && !term.tuned)
                .collect::<Vec<bool>>(),
            Kind::Correction => (self.corrections.iter())
                .filter(|correction| correction.key.like(&key))
                .map(|correction| {
                    correction.original == entry.wrong
                        && correction.replacement == entry.right
                        && correction.matches_case
                })
                .collect(),
        };
        match (like.pop(), like.is_empty()) {
            (None, _) => Held::No,
            (Some(true), true) => Held::Exactly,
            _ => Held::Otherwise,
        }
    }

    /// Takes in an entry that is to be added.
    pub fn add(&mut self, entry: &Entry) {
        match entry.kind {
            Kind::Term => self.terms.push(Term {
                text: entry.right.clone(),
                key: Key::of(&entry.right),
                tuned: false,
            }),
            Kind::Correction => self.corrections.push(Correction {
                original: entry.wrong.clone(),
                replacement: entry.right.clone(),
                matches_case: true,
                key: Key::of(&entry.wrong),
                lower: (entry.wrong.to_lowercase(), entry.right.to_lowercase()),
            }),
        }
    }

    /// Whether nothing is in it.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.corrections.is_empty()
    }
}
