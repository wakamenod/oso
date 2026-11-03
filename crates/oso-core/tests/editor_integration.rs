//! Integration tests for the editor layer.
//!
//! These tests exercise `oso_core::cursor` (and, in future milestones, the
//! buffer and history) through the public API of `oso-core`, which has no
//! wgpu or winit dependency. They can always run in headless / CI
//! environments.

use oso_core::cursor::Cursor;

// ---------------------------------------------------------------------------
// Basic movement scenarios
// ---------------------------------------------------------------------------

#[test]
fn cursor_moves_right_ten_times() {
    let mut c = Cursor::default();
    for _ in 0..10 {
        c.move_right();
    }
    assert_eq!(c.position, 10);
}

#[test]
fn cursor_moves_left_past_zero_stays_at_zero() {
    let mut c = Cursor::default();
    // Move right first so there is something to move back from.
    for _ in 0..5 {
        c.move_right();
    }
    // Now move left more times than we moved right.
    for _ in 0..11 {
        c.move_left();
    }
    assert_eq!(c.position, 0, "position must not underflow below 0");
}

#[test]
fn cursor_right_then_left_symmetric() {
    let mut c = Cursor::default();
    let steps = 7;
    for _ in 0..steps {
        c.move_right();
    }
    for _ in 0..steps {
        c.move_left();
    }
    assert_eq!(c.position, 0);
}

// ---------------------------------------------------------------------------
// Boundary scenarios
// ---------------------------------------------------------------------------

#[test]
fn cursor_at_zero_left_is_noop() {
    let mut c = Cursor::default();
    c.move_left();
    c.move_left();
    assert_eq!(c.position, 0);
}

#[test]
fn cursor_right_from_max_does_not_panic() {
    let mut c = Cursor {
        position: usize::MAX,
    };
    c.move_right(); // saturating: stays at usize::MAX
    assert_eq!(c.position, usize::MAX);
}

// ---------------------------------------------------------------------------
// Sequence scenarios (simulating key-event streams)
// ---------------------------------------------------------------------------

/// Simulate the key sequence: →→→←← and verify final position.
#[test]
fn key_sequence_right_right_right_left_left() {
    let mut c = Cursor::default();
    c.move_right(); // 1
    c.move_right(); // 2
    c.move_right(); // 3
    c.move_left(); // 2
    c.move_left(); // 1
    assert_eq!(c.position, 1);
}

/// Simulate a long alternating sequence and verify position stays consistent.
#[test]
fn alternating_moves_are_consistent() {
    let mut c = Cursor::default();
    for _ in 0..100 {
        c.move_right();
    }
    for _ in 0..50 {
        c.move_left();
    }
    assert_eq!(c.position, 50);
}
