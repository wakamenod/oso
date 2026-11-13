//! A completion component that `%` makes migemo's, as its romaji gives (ADR 0116): its
//! pattern compiled with as many of a dictionary's words as the input's budget leaves, or kept
//! for the same text, case, dictionary and words, and where it matches a candidate, whose kana
//! are composed first.
//!
//! Must not depend on: `editor`, `minibuffer` or `display_map`; it works on strings alone.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

use super::{Candidate, Component, Query, Style};
use crate::migemo::{self, Budget, Dictionary, Spent, Table};

/// Migemo's pattern for `string`, ignoring case if `ignore_case`, with as many of
/// `dictionary`'s words as `budget` leaves, `None` if it gives no kana, and what they spent.
fn compile_migemo(
    string: &str,
    ignore_case: bool,
    dictionary: Option<&Dictionary>,
    budget: Budget,
) -> (Option<Regex>, Spent) {
    let compiled = migemo::compile_within(
        string,
        dictionary,
        migemo::WORDS_SIZE_LIMIT,
        budget,
        |pattern, size_limit| {
            RegexBuilder::new(pattern)
                .case_insensitive(ignore_case)
                .size_limit(size_limit)
                .nest_limit(migemo::NEST_LIMIT)
                .build()
                .ok()
        },
    );
    (compiled.compiled, compiled.spent)
}

impl Component {
    /// The migemo component romaji `rest` stands for, `pattern` as kept, or `None` if it is
    /// not romaji that gives kana, which is then a literal. What a component of `before`, or
    /// one `built` before it of the same input, compiled for the same text, case and table is
    /// kept while `budget` gives it the same words, and it spends what its words do.
    pub(super) fn migemo(
        (rest, pattern): (&str, &str),
        ignore_case: bool,
        (before, built): (&Query, &[Self]),
        (table, budget): (Option<&Table>, &mut Budget),
    ) -> Option<Self> {
        if !migemo::gives_kana(rest) {
            return None;
        }
        let generation = migemo::generation(table);
        let same = (before.components.iter())
            .filter(|_| before.ignore_case == ignore_case)
            .chain(built)
            .find(|old| {
                old.style == Style::Migemo
                    && &*old.pattern == pattern
                    && old.dictionary == generation
                    && old.spent.same_within(*budget)
            });
        let (regex, spent) = match same {
            Some(old) => (old.regex.clone(), old.spent),
            None => {
                let dictionary = table.map(|table| &*table.dictionary);
                compile_migemo(rest, ignore_case, dictionary, *budget)
            }
        };
        budget.spend(spent.spent);
        let regex = regex?;
        Some(Self {
            style: Style::Migemo,
            regex: Some(regex),
            tried: false,
            finder: None,
            chars: rest.chars().count(),
            joined: None,
            not: false,
            annotation: false,
            pattern: pattern.into(),
            dictionary: generation,
            spent,
        })
    }

    /// Where migemo's pattern matches `candidate`'s text, in its byte offsets: in the text
    /// with its kana composed, if it has such a form, mapped back.
    pub(super) fn migemo_find(&self, candidate: &Candidate) -> Option<Range<usize>> {
        let regex = self.regex.as_ref()?;
        match &candidate.composed {
            Some(composed) => {
                let found = regex.find(composed.text.as_deref()?)?;
                Some(composed.original(found.range()))
            }
            None => regex.find(&candidate.text).map(|found| found.range()),
        }
    }
}
