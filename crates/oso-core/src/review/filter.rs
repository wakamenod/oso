//! A review's filter (ADR 0122, "The views", step 6b): the words a person typed after `/`,
//! each of which a file must match, by its path or by one of its comments' texts.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O. The view
//! thread asks which files it shows, and the host what the title and the send echo say.
//!
//! - Words are separated by white space; every word must match the file (Q-C).
//! - Smart case: a word with an upper-case letter matches as typed; any other matches whatever
//!   the case, as isearch does.
//! - A path is matched as its bytes read as UTF-8, invalid bytes standing for themselves only
//!   to a word that holds the same replacement.

use std::sync::Arc;

/// The most bytes of text a filter keeps: a longer one is cut at a character's end.
pub(crate) const MAX_FILTER_BYTES: usize = 1024;

/// What a review shows of its files: those every word matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    inner: Arc<Inner>,
}

#[derive(Debug, PartialEq, Eq)]
struct Inner {
    text: Box<str>,
    words: Box<[Word]>,
}

/// A word to match: as typed, and whether it matches whatever the case, then lower-cased.
#[derive(Debug, PartialEq, Eq)]
struct Word {
    text: Box<str>,
    folded: bool,
}

impl Filter {
    /// The filter `text` says, trimmed and cut at [`MAX_FILTER_BYTES`]; `None` when it holds no
    /// word, which shows every file.
    pub fn new(text: &str) -> Option<Filter> {
        let mut end = text.len().min(MAX_FILTER_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let text = text[..end].trim();
        let words: Box<[Word]> = text
            .split_whitespace()
            .map(|word| {
                let folded = !word.chars().any(char::is_uppercase);
                let text = if folded {
                    word.to_lowercase().into()
                } else {
                    word.into()
                };
                Word { text, folded }
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
    /// Costs the path and the comments' bytes, folded once each when a word asks for it.
    pub fn matches(&self, path: &[u8], comments: &[&str]) -> bool {
        let path = String::from_utf8_lossy(path);
        let folded_path = self
            .inner
            .words
            .iter()
            .any(|word| word.folded)
            .then(|| path.to_lowercase());
        let mut folded_comments: Option<Vec<String>> = None;
        self.inner.words.iter().all(|word| {
            let haystack = |text: &str, folded: Option<&str>| match folded {
                Some(folded) => folded.contains(&*word.text),
                None => text.contains(&*word.text),
            };
            if word.folded {
                if haystack(&path, folded_path.as_deref()) {
                    return true;
                }
                let folded = folded_comments.get_or_insert_with(|| {
                    comments
                        .iter()
                        .map(|comment| comment.to_lowercase())
                        .collect()
                });
                folded.iter().any(|comment| comment.contains(&*word.text))
            } else {
                path.contains(&*word.text)
                    || comments.iter().any(|comment| comment.contains(&*word.text))
            }
        })
    }
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;
