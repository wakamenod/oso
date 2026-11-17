//! Which files a review shows (ADR 0122, step 6b): those every word of its filter matches, by
//! the path, the path before a rename, or a comment's text; and what the title then says of
//! the files it hides.
//!
//! Must not depend on: `app`, the editor or the review's own thread; the view is told the
//! filter, and only lays out the files it shows.
//!
//! - A file's comments are matched only once its reading is known and not withheld: a file
//!   that may hold secrets, one not read yet and one gone are matched by their paths alone,
//!   so that whether one hides never tells of words the review does not show.
//! - Each file keeps its verdict, and what it was judged by: a text built judges again only
//!   the files whose comments, rename or reading changed, or all once the filter changed.
//! - Hidden files keep everything the view holds of them, and come back as the filter goes.

use oso_core::review::{Comment, Filter};

use super::State;
use super::cards::Cards;

/// The filter, how many times it changed, and how many files it hides in the text built last.
#[derive(Default)]
pub(super) struct Filtering {
    filter: Option<Filter>,
    generation: u64,
    hidden: usize,
}

impl Filtering {
    /// Whether a filter hides any file.
    pub(super) fn on(&self) -> bool {
        self.filter.is_some()
    }
}

/// A file's verdict under the filter, with what it was judged by.
#[derive(Default)]
pub(super) struct Judged {
    /// The filter's generation it was judged under; 0 for never.
    generation: u64,
    /// The comments it was judged by, held so that their texts are compared by address, which
    /// a held text keeps; none when they were not matched.
    comments: Vec<Comment>,
    by_comments: bool,
    old: Option<Box<[u8]>>,
    hidden: bool,
}

impl Judged {
    /// Whether the filter hides the file, while there is a filter.
    pub(super) fn hidden(&self) -> bool {
        self.hidden
    }

    /// Whether this verdict was judged by what is the file's now. O(its comments), by address.
    fn holds(&self, generation: u64, old: Option<&[u8]>, by: Option<&[Comment]>) -> bool {
        let same_comments = match by {
            None => !self.by_comments,
            Some(comments) => {
                self.by_comments
                    && comments.len() == self.comments.len()
                    && (comments.iter().zip(&self.comments))
                        .all(|(now, then)| std::ptr::eq(now.text(), then.text()))
            }
        };
        self.generation == generation && self.old.as_deref() == old && same_comments
    }
}

/// Judges the file at `path` again unless its verdict holds; whether it is hidden.
fn judge(
    judged: &mut Judged,
    (filter, generation): (&Filter, u64),
    path: &[u8],
    old: Option<&[u8]>,
    by: Option<&Cards>,
) -> bool {
    let comments = by.map(|cards| cards.comments.as_slice());
    if judged.holds(generation, old, comments) {
        return judged.hidden;
    }
    let old_text = old.map(String::from_utf8_lossy);
    // The path before a rename is one more text each word may match.
    let texts: Vec<&str> = (old_text.as_deref().into_iter())
        .chain(comments.into_iter().flatten().map(Comment::text))
        .collect();
    *judged = Judged {
        generation,
        comments: comments.map(<[Comment]>::to_vec).unwrap_or_default(),
        by_comments: comments.is_some(),
        old: old.map(Box::from),
        hidden: !filter.matches(path, &texts),
    };
    judged.hidden
}

impl State {
    /// Shows only the files `filter` matches from the next text on, or every file with `None`.
    pub(super) fn filter(&mut self, filter: Option<Filter>) {
        if self.filtering.filter != filter {
            self.filtering.filter = filter;
            self.filtering.generation += 1;
            self.dirty = true;
        }
    }

    /// Judges which files the filter hides, before a text is built: again only those whose
    /// verdict no longer holds.
    pub(super) fn refilter(&mut self) {
        let Some(filter) = self.filtering.filter.clone() else {
            self.filtering.hidden = 0;
            return;
        };
        let by = (&filter, self.filtering.generation);
        let mut hidden = 0;
        for (path, file) in &mut self.files {
            let read = file.reading.is_some() || file.pending.is_some();
            let cards = (read && !file.withheld()).then_some(&file.cards);
            let old = file.entry.old_path.as_deref();
            hidden += usize::from(judge(&mut file.judged, by, path, old, cards));
        }
        for (path, gone) in &mut self.gone {
            hidden += usize::from(judge(&mut gone.judged, by, path, None, None));
        }
        self.filtering.hidden = hidden;
    }

    /// What the title says of the filter: how many of the files it hides, and its words, each
    /// once and one space apart, so that none stands out of sight.
    pub(super) fn filter_note(&self) -> Option<String> {
        let filter = self.filtering.filter.as_ref()?;
        let all = self.order.len() + self.gone.len();
        let words: Vec<&str> = filter.text().split_whitespace().collect();
        let quoted = crate::review::one_line(&words.join(" "), MAX_QUOTED);
        Some(format!(
            "{} of {all} files hidden by \"{quoted}\" \u{b7} / to change",
            self.filtering.hidden
        ))
    }
}

/// The most characters of the filter's words the title quotes.
const MAX_QUOTED: usize = 40;
