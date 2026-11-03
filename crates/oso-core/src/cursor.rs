/// Cursor data model (data layer).
///
/// Pixel coordinates (screen_x / screen_y) are computed by the renderer,
/// so this struct holds only the logical position within the text buffer.
#[derive(Default)]
pub struct Cursor {
    /// Byte offset of the cursor in the text buffer (initial value: 0).
    pub position: usize,
}

impl Cursor {
    /// Moves the cursor one position to the right.
    ///
    /// Uses saturating arithmetic so the position never wraps around on overflow.
    pub fn move_right(&mut self) {
        self.position = self.position.saturating_add(1);
    }

    /// Moves the cursor one position to the left (minimum: 0).
    ///
    /// Uses saturating arithmetic so the position never goes below zero.
    pub fn move_left(&mut self) {
        self.position = self.position.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::Cursor;
    use proptest::prelude::*;

    // --- Deterministic unit tests ---

    #[test]
    fn move_right_increments_position() {
        let mut c = Cursor::default();
        c.move_right();
        assert_eq!(c.position, 1);
    }

    #[test]
    fn move_left_at_zero_stays_zero() {
        let mut c = Cursor::default();
        c.move_left();
        assert_eq!(c.position, 0, "underflow must not occur at position 0");
    }

    #[test]
    fn move_right_n_times_equals_n() {
        let mut c = Cursor::default();
        for _ in 0..20 {
            c.move_right();
        }
        assert_eq!(c.position, 20);
    }

    #[test]
    fn move_left_after_right_returns_to_origin() {
        let mut c = Cursor::default();
        c.move_right();
        c.move_right();
        c.move_left();
        c.move_left();
        assert_eq!(c.position, 0);
    }

    #[test]
    fn move_left_excess_clamps_to_zero() {
        let mut c = Cursor { position: 3 };
        for _ in 0..10 {
            c.move_left();
        }
        assert_eq!(c.position, 0);
    }

    #[test]
    fn move_right_at_usize_max_does_not_panic() {
        let mut c = Cursor {
            position: usize::MAX,
        };
        c.move_right(); // saturating_add: stays at usize::MAX
        assert_eq!(c.position, usize::MAX);
    }

    // --- Property-based tests (proptest) ---

    proptest! {
        /// For any starting position, move_left followed by move_right
        /// must never exceed the original position.
        #[test]
        fn prop_left_then_right_never_exceeds_original(pos in 0usize..=usize::MAX) {
            let mut c = Cursor { position: pos };
            c.move_left();
            c.move_right();
            prop_assert!(c.position <= pos);
        }

        /// Repeated move_left from any position must eventually reach 0
        /// and never panic.
        #[test]
        fn prop_repeated_left_converges_to_zero(pos in 0usize..1000usize) {
            let mut c = Cursor { position: pos };
            for _ in 0..=pos {
                c.move_left();
            }
            prop_assert_eq!(c.position, 0);
        }

        /// move_right from any position below usize::MAX must increase position by 1.
        #[test]
        fn prop_move_right_increments_below_max(pos in 0usize..usize::MAX) {
            let mut c = Cursor { position: pos };
            c.move_right();
            prop_assert_eq!(c.position, pos + 1);
        }
    }
}
