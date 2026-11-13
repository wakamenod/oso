//! A completion component that `%` makes migemo's, as its romaji gives (ADR 0116): its
//! pattern compiled with a dictionary's words, or kept for the same text, case and dictionary,
//! and where it matches a candidate, whose kana are composed first.
//!
//! Must not depend on: `editor`, `minibuffer` or `display_map`; it works on strings alone.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

use super::{Candidate, Component, Query, Style};
use crate::migemo::{self, Dictionary, Table};

/// Migemo's pattern for `string`, ignoring case if `ignore_case`, with `dictionary`'s words,
/// `None` if it gives no kana.
fn compile_migemo(
    string: &str,
    ignore_case: bool,
    dictionary: Option<&Dictionary>,
) -> Option<Regex> {
    migemo::compile(
        string,
        dictionary,
        migemo::WORDS_SIZE_LIMIT,
        |pattern, size_limit| {
            RegexBuilder::new(pattern)
                .case_insensitive(ignore_case)
                .size_limit(size_limit)
                .nest_limit(migemo::NEST_LIMIT)
                .build()
                .ok()
        },
    )
}

impl Component {
    /// The migemo component romaji `rest` stands for, `pattern` as kept, or `None` if it is
    /// not romaji that gives kana, which is then a literal. What a component of `before`, or
    /// one `built` before it of the same input, compiled for the same text, case and table is
    /// kept.
    pub(super) fn migemo(
        (rest, pattern): (&str, &str),
        ignore_case: bool,
        (before, built): (&Query, &[Self]),
        table: Option<&Table>,
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
            });
        let regex = match same {
            Some(old) => old.regex.clone(),
            None => compile_migemo(rest, ignore_case, table.map(|t| &*t.dictionary)),
        }?;
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
