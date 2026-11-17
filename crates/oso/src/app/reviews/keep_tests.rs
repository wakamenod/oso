//! The tests of keeping reviews that need no thread or window: when a change is saved.
//!
//! Must not depend on: the store's thread, winit or the renderer.

use std::time::{Duration, Instant};

use super::{AT_MOST, QUIET, Saving, due_at};

#[test]
fn a_change_is_saved_once_it_settles_or_at_most_a_while_after_the_first() {
    let start = Instant::now();
    let mut saving = Saving::default();
    assert_eq!(due_at(&saving), None, "nothing changed");
    saving.first = Some(start);
    saving.last = Some(start);
    assert_eq!(due_at(&saving), Some(start + QUIET));
    // Changes go on: the last one moves the time on, up to the most after the first.
    saving.last = Some(start + Duration::from_millis(900));
    assert_eq!(
        due_at(&saving),
        Some(start + Duration::from_millis(900) + QUIET)
    );
    saving.last = Some(start + AT_MOST);
    assert_eq!(due_at(&saving), Some(start + AT_MOST));
}
