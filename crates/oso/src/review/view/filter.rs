//! Which files a review shows (ADR 0122, step 6b): those every word of its filter matches, by
//! the path, the path before a rename, or a comment's text, judged once per text built; and
//! what the title then says of the files it hides.
//!
//! Must not depend on: `app`, the editor or the review's own thread; the view is told the
//! filter, and only lays out the files it shows.
//!
//! - A file that may hold secrets, whose lines no reading holds, is matched by its path alone,
//!   so that what hides never tells of its comments' words (Q-D).
//! - Hidden files keep everything the view holds of them, and come back as the filter goes.

use std::collections::HashSet;

use oso_core::review::Filter;

use super::State;
use super::cards::Cards;

/// The filter and the files it hides in the text built last.
#[derive(Default)]
pub(super) struct Filtering {
    filter: Option<Filter>,
    hidden: HashSet<Box<[u8]>>,
}

/// The most characters of the filter's text the title quotes.
const MAX_QUOTED: usize = 40;

impl State {
    /// Shows only the files `filter` matches from the next text on, or every file with `None`.
    pub(super) fn filter(&mut self, filter: Option<Filter>) {
        if self.filtering.filter != filter {
            self.filtering.filter = filter;
            self.dirty = true;
        }
    }

    /// Judges again which files the filter hides, before a text is built.
    pub(super) fn refilter(&mut self) {
        let Some(filter) = &self.filtering.filter else {
            self.filtering.hidden.clear();
            return;
        };
        let shows = |path: &[u8], old: Option<&[u8]>, cards: &Cards, withheld: bool| {
            let comments: Vec<&str> = if withheld {
                Vec::new()
            } else {
                cards
                    .comments
                    .iter()
                    .map(|comment| comment.text())
                    .collect()
            };
            filter.matches(path, &comments) || old.is_some_and(|old| filter.matches(old, &comments))
        };
        let mut hidden = HashSet::new();
        for (path, file) in &self.files {
            let old = file.entry.old_path.as_deref();
            if !shows(path, old, &file.cards, file.withheld()) {
                hidden.insert(path.clone());
            }
        }
        for (path, gone) in &self.gone {
            if !shows(path, None, &gone.cards, gone.withheld) {
                hidden.insert(path.clone());
            }
        }
        self.filtering.hidden = hidden;
    }

    /// Whether the text built shows the file at `path`.
    pub(super) fn shows(&self, path: &[u8]) -> bool {
        !self.filtering.hidden.contains(path)
    }

    /// What the title says of the filter: how many of the files it hides, and its words.
    pub(super) fn filter_note(&self) -> Option<String> {
        let filter = self.filtering.filter.as_ref()?;
        let all = self.order.len() + self.gone.len();
        let quoted: String = (filter.text().chars())
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(MAX_QUOTED)
            .collect();
        let more = if filter.text().chars().nth(MAX_QUOTED).is_some() {
            "\u{2026}"
        } else {
            ""
        };
        Some(format!(
            "{} of {all} files hidden by \"{quoted}{more}\" \u{b7} / to change",
            self.filtering.hidden.len()
        ))
    }
}
