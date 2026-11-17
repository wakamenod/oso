//! Which files a review's view holds (ADR 0122, "The views"): a listing's, in its order, and
//! after them those gone from it that still have comments, as the model's cards say.
//!
//! Must not depend on: `app`, the editor or the review's own thread.

use super::cards::Cards;
use super::file::{File, Gone};
use super::{Entry, State};

impl State {
    /// Takes a new listing. A file that left a whole listing with comments stays, after the
    /// others, as no longer changed. A cut listing tells nothing of the files it left out, as
    /// the model's files keep them: one with comments stays shown as it was, after the listed
    /// ones.
    pub(super) fn listed(&mut self, entries: &[Entry], more: usize, cut: bool) {
        let whole = more == 0 && !cut;
        let before = std::mem::take(&mut self.order);
        let mut files = std::mem::take(&mut self.files);
        self.order = self.held.listed(self.highlighting.top(), entries);
        for entry in entries {
            let file = match files.remove(&entry.path) {
                Some(file) => file,
                None => {
                    let cards = self.gone.remove(&entry.path).map(|gone| gone.cards);
                    File::new(entry.clone(), cards.unwrap_or_default())
                }
            };
            self.files.insert(entry.path.clone(), file);
        }
        // In the order they were shown.
        for path in &before {
            let Some(file) = files.remove(path) else {
                continue;
            };
            if file.cards.comments.is_empty() {
                self.highlighting.forget(path);
                continue;
            }
            if whole {
                self.highlighting.forget(path);
                let gone = Gone {
                    withheld: self.held.left(self.highlighting.top(), &file),
                    cards: file.cards,
                    ..Gone::default()
                };
                self.gone.insert(path.clone(), gone);
                self.dirty = true;
            } else {
                self.order.push(path.clone());
                self.files.insert(path.clone(), file);
            }
        }
        self.highlighting.keep_listed_buffers();
        let same = self.more == more && self.cut == cut && before == self.order;
        self.dirty |= !same;
        self.more = more;
        self.cut = cut;
        self.reading = true;
    }

    /// Takes the model's snapshot of a file's comments. A file not listed keeps a section while
    /// it has any.
    pub(super) fn cards(&mut self, path: Box<[u8]>, cards: Cards) {
        if let Some(file) = self.files.get_mut(&path) {
            file.cards = cards;
            file.rendered.clear();
            self.dirty = true;
        } else if cards.comments.is_empty() {
            self.dirty |= self.gone.remove(&path).is_some();
        } else {
            let withheld = self.held.told(self.highlighting.top(), &path);
            let gone = Gone {
                cards,
                withheld,
                ..Gone::default()
            };
            self.gone.insert(path, gone);
            self.dirty = true;
        }
    }
}
