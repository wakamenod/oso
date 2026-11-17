//! A review's filter (ADR 0122, "The views", step 6b): the words a person typed after `/`,
//! each of which a file must match, by its path or by one of its comments' texts.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O. The view
//! thread asks which files it shows, and the host what the title and the send echo say.
//!
//! - Words are separated by white space; every word must match the file (Q-C).
//! - Smart case: a word with an upper-case letter matches as typed; any other matches whatever
//!   the case, as isearch does.
//! - A path is matched as its bytes, a word as UTF-8: an invalid byte matches no word.
//! - Each word is a literal `regex` (ADR 0076's crate) built once, so matching folds and
//!   allocates nothing, in time linear in the path and the comments; at most [`MAX_WORDS`]
//!   words are kept.

use std::sync::Arc;

use regex::bytes::{Regex, RegexBuilder};

/// The most bytes of text a filter keeps: a longer one is cut at a character's end.
const MAX_FILTER_BYTES: usize = 1024;

/// The most words a filter matches by: those after are left out, so that building one costs
/// at most this many small regexes.
const MAX_WORDS: usize = 32;

/// What a review shows of its files: those every word matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    text: Box<str>,
    /// Each word, as a literal matched as typed or whatever the case.
    words: Box<[Regex]>,
}

/// Two filters are the same when their texts are: the words follow from the text.
impl PartialEq for Inner {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for Inner {}

impl Filter {
    /// The filter `text` says, cut at [`MAX_FILTER_BYTES`], then trimmed; `None` when it holds
    /// no word, which shows every file.
    pub fn new(text: &str) -> Option<Filter> {
        let mut end = text.len().min(MAX_FILTER_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let text = text[..end].trim();
        let words: Box<[Regex]> = (text.split_whitespace())
            .take(MAX_WORDS)
            .filter_map(|word| {
                let folded = !word.chars().any(char::is_uppercase);
                RegexBuilder::new(&regex::escape(word))
                    .case_insensitive(folded)
                    .build()
                    .ok()
            })
            .collect();
        (!words.is_empty()).then(|| Filter {
            inner: Arc::new(Inner {
                text: text.into(),
                words,
            }),
        })
    }

    /// The text as typed, trimmed: what the prompt is filled with and the title quotes.
    pub fn text(&self) -> &str {
        &self.inner.text
    }

    /// Whether every word matches the file at `path`, by the path or by one of `comments`.
    /// Costs a pass over the path and the comments a word, allocating nothing.
    pub fn matches(&self, path: &[u8], comments: &[&str]) -> bool {
        self.inner.words.iter().all(|word| {
            word.is_match(path)
                || comments
                    .iter()
                    .any(|comment| word.is_match(comment.as_bytes()))
        })
    }
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;
