//! One file's place in a review's view (ADR 0122, "The views"): the reading it shows, a newer
//! one waiting for its comments to be carried, the model's snapshot of its comments, and its
//! section as last laid out, kept while laying it out again would give the same.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; this keeps what the view
//! thread is told.

use std::sync::Arc;

use oso_core::review::{Anchor, CommentId};

use super::cards::Cards;
use super::panes;
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
    pub(super) rendered: Laid,
    /// Whether the filter hides it, and its row in the files pane (step 6b).
    pub(super) judged: super::filter::Judged,
    pub(super) pane_row: Option<super::files::PaneRow>,
}

/// A file's section and its two panes as last laid out, each kept while laying it out again
/// would give the same, and cleared together.
#[derive(Default)]
pub(super) struct Laid {
    pub(super) unified: Option<Kept<section::Rendered>>,
    pub(super) panes: Option<Kept<panes::Sides>>,
}

impl Laid {
    pub(super) fn clear(&mut self) {
        *self = Laid::default();
    }
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
            rendered: Laid::default(),
            judged: super::filter::Judged::default(),
            pane_row: None,
        }
    }

    /// Shows `reading`, its comments placed by `placed`, and drops a carry it waited for.
    pub(super) fn show(&mut self, reading: Arc<Reading>, placed: Option<Placed>, cut_short: bool) {
        self.entry = reading.entry.clone();
        self.reading = Some(reading);
        self.pending = None;
        self.placed = placed;
        self.cut_short = cut_short;
        self.rendered.clear();
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

    /// How its comments show beside its reading.
    fn shown(&self, folded: bool) -> section::Shown<'_> {
        section::Shown {
            cards: &self.cards,
            placed: self.placed.as_deref(),
            cut_short: self.cut_short,
            folded,
        }
    }

    /// Its section, given the bytes the review's text has room for.
    pub(super) fn render(&self, room: usize, folded: bool) -> section::Rendered {
        match &self.reading {
            Some(reading) => section::render(reading, room, &self.shown(folded)),
            None => section::unread(&self.entry, &self.cards),
        }
    }

    /// Its two panes, given the bytes the review's text has room for.
    pub(super) fn render_panes(&self, room: usize, folded: bool) -> panes::Sides {
        match &self.reading {
            Some(reading) => panes::render(reading, room, &self.shown(folded)),
            None => panes::unread(&self.entry, &self.cards),
        }
    }
}

/// A file no longer listed whose comments stay to be seen: only counted if its last reading
/// was withheld, as a withheld file's are.
#[derive(Default)]
pub(super) struct Gone {
    pub(super) cards: Cards,
    pub(super) withheld: bool,
    pub(super) rendered: Laid,
    pub(super) judged: super::filter::Judged,
    pub(super) pane_row: Option<super::files::PaneRow>,
}

/// What was laid out for a file, as its cache keeps it: whether it holds a card's row, whether a
/// card showed its label alone, and its bytes.
pub(super) trait Laidout {
    fn cards(&self) -> bool;
    fn cut(&self) -> bool;
    fn len(&self) -> usize;
}

impl Laidout for section::Rendered {
    fn cards(&self) -> bool {
        self.cards
    }

    fn cut(&self) -> bool {
        self.cut
    }

    fn len(&self) -> usize {
        self.bytes.len()
    }
}

/// A file's section or panes as last laid out, with the room the text had for them.
pub(super) struct Kept<T> {
    pub(super) rendered: Arc<T>,
    room: usize,
}

impl<T: Laidout> Kept<T> {
    /// What was laid out, if laying it out with `room` gives the same: the same room, any room
    /// but none for what holds no card, or room for all of it when no card was cut.
    fn fits(&self, room: usize) -> Option<Arc<T>> {
        let laid = &*self.rendered;
        let same = (self.room == 0) == (room == 0)
            && (self.room == room || !laid.cards() || (!laid.cut() && room >= laid.len()));
        same.then(|| Arc::clone(&self.rendered))
    }
}

/// `kept`'s section or panes if they fit `room`, else `lay_out`'s, kept for the next text.
pub(super) fn section_in<T: Laidout>(
    kept: &mut Option<Kept<T>>,
    room: usize,
    lay_out: impl FnOnce() -> T,
) -> Arc<T> {
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
