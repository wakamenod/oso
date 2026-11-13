//! One file's place in a review's view (ADR 0122, "The views"): the reading it shows, a newer
//! one waiting for its comments to be carried, the model's snapshot of its comments, and its
//! section as last laid out, kept while laying it out again would give the same.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; this keeps what the view
//! thread is told.

use std::sync::Arc;

use oso_core::review::{Anchor, CommentId};

use super::cards::Cards;
use super::section;
use crate::review::{Content, Entry, Reading};

/// Where a carry placed each comment, by id.
pub(super) type Placed = Box<[(CommentId, Option<Anchor>)]>;

/// One file's place in the view.
pub(super) struct File {
    pub(super) entry: Entry,
    /// The reading shown.
    pub(super) reading: Option<Arc<Reading>>,
    /// A newer reading whose comments the review's thread is carrying.
    pub(super) pending: Option<Arc<Reading>>,
    /// Where the last carry to the reading shown placed its comments, and whether it ran out
    /// of time.
    placed: Option<Placed>,
    cut_short: bool,
    pub(super) cards: Cards,
    pub(super) rendered: Option<Kept>,
}

impl File {
    pub(super) fn new(entry: Entry, cards: Cards) -> File {
        File {
            entry,
            reading: None,
            pending: None,
            placed: None,
            cut_short: false,
            cards,
            rendered: None,
        }
    }

    /// Shows `reading`, its comments placed by `placed`, and drops a carry it waited for.
    pub(super) fn show(&mut self, reading: Arc<Reading>, placed: Option<Placed>, cut_short: bool) {
        self.entry = reading.entry.clone();
        self.reading = Some(reading);
        self.pending = None;
        self.placed = placed;
        self.cut_short = cut_short;
        self.rendered = None;
    }

    /// The reading a carry must be for: the one waiting for its carry, else the one shown.
    pub(super) fn current(&self) -> Option<&Arc<Reading>> {
        self.pending.as_ref().or(self.reading.as_ref())
    }

    /// Whether the reading shown, or the one waiting, was withheld as it may hold secrets.
    pub(super) fn withheld(&self) -> bool {
        [&self.reading, &self.pending]
            .into_iter()
            .flatten()
            .any(|reading| matches!(reading.content, Content::Withheld))
    }

    /// Its section, given the bytes the review's text has room for.
    pub(super) fn render(&self, room: usize, folded: bool) -> section::Rendered {
        match &self.reading {
            Some(reading) => section::render(
                reading,
                room,
                &section::Shown {
                    cards: &self.cards,
                    placed: self.placed.as_deref(),
                    cut_short: self.cut_short,
                    folded,
                },
            ),
            None => section::unread(&self.entry, &self.cards),
        }
    }
}

/// A file no longer listed whose comments stay to be seen: only counted if its last reading
/// was withheld, as a withheld file's are.
pub(super) struct Gone {
    pub(super) cards: Cards,
    pub(super) withheld: bool,
    pub(super) rendered: Option<Kept>,
}

/// A section as last laid out, with the room the text had for it.
pub(super) struct Kept {
    pub(super) rendered: Arc<section::Rendered>,
    room: usize,
}

impl Kept {
    /// Its section, if laying it out with `room` gives the same: the same room, any room but
    /// none for a section with no card, or room for all of it when no card was cut.
    fn fits(&self, room: usize) -> Option<Arc<section::Rendered>> {
        let rendered = &self.rendered;
        let same = (self.room == 0) == (room == 0)
            && (self.room == room
                || !rendered.cards
                || (!rendered.cut && room >= rendered.bytes.len()));
        same.then(|| Arc::clone(&self.rendered))
    }
}

/// `kept`'s section if it fits `room`, else `lay_out`'s, kept for the next text.
pub(super) fn section_in(
    kept: &mut Option<Kept>,
    room: usize,
    lay_out: impl FnOnce() -> section::Rendered,
) -> Arc<section::Rendered> {
    let rendered = kept
        .as_ref()
        .and_then(|kept| kept.fits(room))
        .unwrap_or_else(|| Arc::new(lay_out()));
    *kept = Some(Kept {
        rendered: Arc::clone(&rendered),
        room,
    });
    rendered
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Kept, section};

    #[test]
    fn a_section_with_no_card_fits_any_room_but_none() {
        let kept = Kept {
            rendered: Arc::new(section::title("t", &[])),
            room: 1000,
        };
        assert!(kept.fits(1).is_some());
        assert!(kept.fits(usize::MAX).is_some());
        assert!(kept.fits(0).is_none());
    }
}
