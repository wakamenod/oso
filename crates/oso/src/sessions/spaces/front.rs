//! The space in front after a restart's windows are in (ADR 0136): the frame may have been given to
//! a space as the record was read, before them.
//!
//! Must not depend on: `app`, `dispatch` or `config`; it is `Spaces`' bookkeeping.

use super::{Space, Spaces};

impl Spaces {
    /// Space `at` is the one whose tree is in the frame, in place of the one that adopted it, with
    /// the trees kept behind it: a restart put its windows in after a space was adopted. Nothing
    /// once a space's tree was swapped in since. Whether it changed.
    pub fn adopt_instead(&mut self, at: usize) -> bool {
        let open = self.spaces.get(at).is_some_and(Space::is_open);
        let Some(from) = self.current.filter(|from| *from != at) else {
            return false;
        };
        if !open || self.entries != 1 {
            return false;
        }
        if let Some(space) = self.spaces.get_mut(from) {
            space.entered = false;
            space.stamp = 0;
            self.kept = std::mem::take(&mut space.kept);
        }
        self.current = None;
        self.entries = 0;
        self.adopt(at);
        true
    }
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Buffer;

    use super::super::tests::{add, editor, shown};
    use super::Spaces;

    #[test]
    fn a_restarts_space_is_in_front_in_place_of_the_one_that_adopted_the_frame() {
        let mut editor = editor();
        let mut spaces = Spaces::default();
        let one = add(&mut spaces, "/src/one");
        let two = add(&mut spaces, "/src/two");
        assert!(!spaces.adopt_instead(two), "none adopted the frame");
        spaces.adopt(one);
        spaces.zoom(&mut editor).unwrap();
        assert!(!spaces.adopt_instead(one), "in front already");
        assert!(spaces.adopt_instead(two));
        assert_eq!(spaces.current(), Some(two));
        assert_eq!(spaces.kept().len(), 1, "the zoom goes with the frame");
        // The other was never entered: it is built from its last buffers.
        let file = editor.add_buffer(Buffer::from_text("one.rs", "1"));
        spaces.showed(one, None, Some(file));
        spaces.enter(&mut editor, one).unwrap();
        assert_eq!(shown(&editor), ["one.rs"]);
        // The person is in a space they entered: it stays.
        assert!(!spaces.adopt_instead(two));
        assert_eq!(spaces.current(), Some(one));
    }
}
