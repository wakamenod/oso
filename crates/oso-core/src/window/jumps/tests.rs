//! Tests of a window's history of jumps: back and forward, its bound, and places that follow
//! edits and go with their buffer.
//!
//! Must not depend on: the editor; these drive the history alone.

use super::*;

fn id(n: u64) -> BufferId {
    BufferId(n)
}

#[test]
fn back_and_forward_walk_the_places_and_a_new_jump_ends_the_way_forward() {
    let mut jumps = Jumps::default();
    jumps.push((id(1), 10));
    jumps.push((id(2), 20));
    assert_eq!(jumps.back((id(3), 30)), Some((id(2), 20)));
    assert_eq!(jumps.back((id(2), 20)), Some((id(1), 10)));
    assert_eq!(jumps.back((id(1), 10)), None);
    assert_eq!(jumps.forward((id(1), 10)), Some((id(2), 20)));
    assert_eq!(jumps.forward((id(2), 20)), Some((id(3), 30)));
    assert_eq!(jumps.forward((id(3), 30)), None);
    assert_eq!(jumps.back((id(3), 31)), Some((id(2), 20)));
    jumps.push((id(2), 22));
    assert_eq!(jumps.counts(), (2, 0), "a new jump empties the way forward");
}

#[test]
fn past_the_bound_the_oldest_goes() {
    let mut jumps = Jumps::default();
    for n in 0..JUMPS_MAX + 3 {
        jumps.push((id(1), n));
    }
    assert_eq!(jumps.counts(), (JUMPS_MAX, 0));
    let mut last = None;
    while let Some(place) = jumps.back((id(1), 0)) {
        last = Some(place);
    }
    assert_eq!(last, Some((id(1), 3)));
}

#[test]
fn places_follow_their_buffer_s_edits_and_a_kill_lets_them_go() {
    let mut jumps = Jumps::default();
    jumps.push((id(1), 10));
    jumps.push((id(2), 10));
    jumps.push((id(1), 5));
    let edits = vec![OffsetEdit {
        start: 5,
        old_len: 0,
        new_len: 3,
    }];
    jumps.follow(id(1), &[edits]);
    assert_eq!(
        jumps.back((id(3), 0)),
        Some((id(1), 5)),
        "before what was inserted"
    );
    assert_eq!(
        jumps.back((id(1), 5)),
        Some((id(2), 10)),
        "another buffer's stays"
    );
    assert_eq!(jumps.back((id(2), 10)), Some((id(1), 13)));
    jumps.forget(id(2));
    assert_eq!(jumps.counts(), (0, 2));
    assert_eq!(format!("{jumps:?}"), "Jumps { back: 0, forward: 2 }");
}
