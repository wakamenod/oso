//! Where a window was in each buffer it showed, the order it showed them, and the way back
//! through the internal buffers it went to (ADR 0088).
//!
//! Must not depend on: `editor` or `command`.
//!
//! Leaving a buffer keeps its place: the selection set, with every cursor, the marks and
//! the mark ring, and the view. Coming back gives it back, for the editor to clip to the
//! text as it is then. The places are kept most recent first, which is the order `C-x b`
//! lists buffers in and `M-,` walks.

use crate::buffer::BufferId;
use crate::selection::SelectionSet;

use super::{Scroll, Window};

/// Where a window was in a buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The cursors, their marks and the mark ring.
    pub selections: SelectionSet,
    /// What the window showed.
    pub scroll: Scroll,
}

/// The places of the buffers a window showed before the one it shows, most recent first.
#[derive(Clone, Debug, Default)]
pub(crate) struct Places {
    order: Vec<(BufferId, Place)>,
    /// While `M-,` and `M-/` walk the buffers: the buffers in the order they walk, and
    /// where in it the window is. Anything else that shows a buffer ends it.
    walk: Option<Walk>,
}

/// A walk through the buffers with `M-,` and `M-/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Walk {
    /// The buffers, the one the walk started from first.
    pub(crate) ring: Vec<BufferId>,
    /// Where the window is in `ring`.
    pub(crate) at: usize,
}

impl Places {
    /// Keeps `place` as where the window was in `buffer`, as the most recent.
    pub(crate) fn leave(&mut self, buffer: BufferId, place: Place) {
        self.order.retain(|(other, _)| *other != buffer);
        self.order.insert(0, (buffer, place));
    }

    /// Takes out where the window was in `buffer`, as it shows it again.
    pub(crate) fn take(&mut self, buffer: BufferId) -> Option<Place> {
        let index = self.order.iter().position(|(other, _)| *other == buffer)?;
        Some(self.order.remove(index).1)
    }

    /// Where the window was in `buffer`, to change, as a list changed behind it.
    pub(crate) fn get_mut(&mut self, buffer: BufferId) -> Option<&mut Place> {
        self.order
            .iter_mut()
            .find_map(|(other, place)| (*other == buffer).then_some(place))
    }

    /// Forgets `buffer`, which is gone.
    pub(crate) fn forget(&mut self, buffer: BufferId) {
        self.order.retain(|(other, _)| *other != buffer);
        if let Some(walk) = &mut self.walk {
            if walk.ring.get(walk.at) == Some(&buffer) {
                self.walk = None;
            } else if let Some(index) = walk.ring.iter().position(|other| *other == buffer) {
                walk.ring.remove(index);
                if index < walk.at {
                    walk.at -= 1;
                }
            }
        }
    }

    /// The buffers the window showed before, most recent first, with where it was in each.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (BufferId, &Place)> {
        self.order.iter().map(|(buffer, place)| (*buffer, place))
    }

    /// The walk under way, if the window still shows where it left off.
    pub(crate) fn walk(&self, shown: BufferId) -> Option<&Walk> {
        self.walk
            .as_ref()
            .filter(|walk| walk.ring.get(walk.at) == Some(&shown))
    }

    /// Starts or goes on with a walk, which [`Places::end_walk`] ends.
    pub(crate) fn set_walk(&mut self, walk: Walk) {
        self.walk = Some(walk);
    }

    /// Ends a walk, as something other than `M-,` or `M-/` showed a buffer.
    pub(crate) fn end_walk(&mut self) {
        self.walk = None;
    }
}

impl Window {
    /// Shows `buffer`, keeping where the window was in the one it leaves, and dropping its
    /// wrap of it (ADR 0103). Gives where it was in `buffer` the last time, for the editor to
    /// clip to the text as it is now; until then it shows `buffer` from its start, with one
    /// cursor. Showing the buffer it shows changes nothing there. Ends a walk of `M-,` and
    /// `M-/`.
    pub(crate) fn show(&mut self, buffer: BufferId) -> Option<Place> {
        self.places.end_walk();
        if buffer == self.buffer {
            return None;
        }
        let mut places = std::mem::take(&mut self.places);
        let place = Place {
            selections: std::mem::take(&mut self.selections),
            scroll: self.scroll,
        };
        places.leave(self.buffer, place);
        let place = places.take(buffer);
        *self = Self {
            size: self.size,
            places,
            gutter_floor: self.gutter_floor,
            way_back: std::mem::take(&mut self.way_back),
            ..Self::new(self.id, buffer)
        };
        place
    }

    /// Whether it has shown `buffer`: it shows it, or keeps where it was in it.
    pub(crate) fn has_shown(&self, buffer: BufferId) -> bool {
        self.buffer == buffer || self.places.iter().any(|(id, _)| id == buffer)
    }

    /// The buffers it was on before each internal buffer it has gone to, the first first.
    pub(crate) fn way_back(&self) -> &[BufferId] {
        &self.way_back
    }

    pub(crate) fn way_back_mut(&mut self) -> &mut Vec<BufferId> {
        &mut self.way_back
    }

    /// The buffers the window showed before the one it shows, most recent first, with where it
    /// was in each.
    pub fn places(&self) -> impl Iterator<Item = (BufferId, &Place)> {
        self.places.iter()
    }

    pub(crate) fn places_mut(&mut self) -> &mut Places {
        &mut self.places
    }

    pub(crate) fn walk(&self) -> Option<&Walk> {
        self.places.walk(self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> BufferId {
        BufferId(n)
    }

    fn place(cursor: usize) -> Place {
        Place {
            selections: SelectionSet::new(cursor),
            scroll: Scroll::default(),
        }
    }

    #[test]
    fn places_come_back_most_recent_first_and_once() {
        let mut places = Places::default();
        places.leave(id(1), place(1));
        places.leave(id(2), place(2));
        places.leave(id(1), place(3));
        let order: Vec<_> = places.iter().map(|(buffer, _)| buffer).collect();
        assert_eq!(order, [id(1), id(2)]);
        assert_eq!(places.take(id(1)), Some(place(3)));
        assert_eq!(places.take(id(1)), None);
        places.forget(id(2));
        assert_eq!(places.iter().count(), 0);
    }

    #[test]
    fn a_walk_holds_while_the_window_is_where_it_left_off() {
        let mut places = Places::default();
        places.set_walk(Walk {
            ring: vec![id(1), id(2), id(3)],
            at: 2,
        });
        assert!(places.walk(id(3)).is_some());
        assert!(
            places.walk(id(2)).is_none(),
            "something else showed another"
        );
        places.forget(id(1));
        assert_eq!(
            places.walk(id(3)),
            Some(&Walk {
                ring: vec![id(2), id(3)],
                at: 1
            })
        );
        places.forget(id(3));
        assert!(places.walk(id(3)).is_none(), "the buffer it was on went");
    }
}
