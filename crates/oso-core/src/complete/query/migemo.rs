//! A completion component that `%` makes migemo's (ADR 0116): its pattern compiled with a
//! dictionary's words, or kept from the input before for the same text, case and dictionary,
//! and where it matches a candidate, whose kana are composed first.
//!
//! Must not depend on: `editor`, `minibuffer` or `display_map`; it works on strings alone.

use std::ops::Range;
use std::sync::Arc;

use regex::Regex;

use super::{Candidate, Component, Query, Style, compile_within};
use crate::migemo::{self, Dictionary};

/// Whether `a` and `b` are the same dictionary, or both none.
pub(super) fn same_dictionary(a: Option<&Arc<Dictionary>>, b: Option<&Arc<Dictionary>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

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
        |pattern, size_limit| compile_within(pattern, ignore_case, size_limit),
    )
}

impl Component {
    /// The migemo component romaji `rest` stands for, `pattern` as kept, or `None` if it is
    /// not romaji that gives kana, which is then a literal.
    pub(super) fn migemo(
        rest: &str,
        pattern: &str,
        ignore_case: bool,
        before: &Query,
        dictionary: Option<&Arc<Dictionary>>,
    ) -> Option<Self> {
        if !migemo::is_romaji(rest) {
            return None;
        }
        let same = before
            .components
            .iter()
            .filter(|_| before.ignore_case == ignore_case)
            .find(|old| {
                old.style == Style::Migemo
                    && &*old.pattern == pattern
                    && same_dictionary(old.dictionary.as_ref(), dictionary)
            });
        let regex = match same {
            Some(old) => old.regex.clone(),
            None => compile_migemo(rest, ignore_case, dictionary.map(|d| &**d)),
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
            dictionary: dictionary.cloned(),
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
