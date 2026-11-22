//! The trees kept behind a frame's: each space keeps its own, the review layout's entry goes
//! with its space, and a zoom nests inside the layout (ADR 0160).
//!
//! Must not depend on: `app` or `dispatch`; these drive an editor alone, with no window.

use std::path::Path;

use oso_core::buffer::Buffer;

use super::super::{Member, SpaceId, Spaces, Stored, Zoom};
use super::{self as kept, Kept, ReviewKept};

/// A space of the one repository at `root`, recalled.
fn add(spaces: &mut Spaces, root: &str) -> usize {
    let n = u8::try_from(spaces.iter().count()).unwrap() + 1;
    let name = Path::new(root)
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let stored = Stored::of_repository(
        SpaceId::from_random([n; 16]),
        name,
        Member::at(Path::new(root)),
        1,
    );
    spaces.recall(vec![stored], Vec::new(), false);
    spaces.iter().count() - 1
}

#[test]
fn each_space_keeps_the_trees_behind_its_own_and_a_review_layout_goes_with_it() {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let (review, _) = editor.add_review(project, "*review: uncommitted oso*".into());
    let mut spaces = Spaces::default();
    let one = add(&mut spaces, "/src/one");
    let two = add(&mut spaces, "/src/two");
    spaces.adopt(one);
    let window = editor.window().id();
    let before = editor.lone_window_tree();
    spaces.kept_mut().push(Kept::Review(ReviewKept {
        before: Some(before),
        review,
        window,
        slot: None,
        from: None,
        ask: None,
        asked: None,
    }));
    let file = editor.add_buffer(Buffer::from_text("b.rs", "b"));
    spaces.showed(two, None, Some(file));
    spaces.enter(&mut editor, two).unwrap();
    assert!(spaces.kept().is_empty(), "the other space holds none");
    // A zoom in the other space is its own.
    assert_eq!(spaces.zoom(&mut editor).map(|(zoom, _)| zoom), Ok(Zoom::In));
    spaces.enter(&mut editor, one).unwrap();
    assert!(
        kept::review_in_frame(spaces.kept()),
        "the layout came back with its space"
    );
    assert_eq!(spaces.zoom(&mut editor).map(|(zoom, _)| zoom), Ok(Zoom::In));
    assert!(
        !kept::review_in_frame(spaces.kept()),
        "a zoom inside the layout"
    );
    assert!(kept::review(spaces.kept()).is_some());
    assert_eq!(
        spaces.zoom(&mut editor).map(|(zoom, _)| zoom),
        Ok(Zoom::Out)
    );
    assert!(kept::review_in_frame(spaces.kept()));
    spaces.enter(&mut editor, two).unwrap();
    assert!(matches!(spaces.kept(), [Kept::Zoom(_)]));
}
