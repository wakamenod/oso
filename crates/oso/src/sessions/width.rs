//! The width of `*Sessions*`' pane (ADR 0131, Q7): 44 columns unless `[sessions] pane_width`
//! says otherwise, from 24 to 80, and `<` and `>` change it for this oso instance.
//!
//! Must not depend on: `app` or the frame; plain numbers.

use std::ops::RangeInclusive;

/// The pane's width unless `[sessions] pane_width` gives one.
pub const DEFAULT: usize = 44;

/// The widths `[sessions] pane_width`, `<` and `>` keep to.
pub const WIDTHS: RangeInclusive<usize> = 24..=80;

/// `width` changed by `columns`, narrower if negative, kept within [`WIDTHS`].
pub fn changed(width: usize, columns: isize) -> usize {
    width
        .saturating_add_signed(columns)
        .clamp(*WIDTHS.start(), *WIDTHS.end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_keeps_within_the_widths() {
        assert_eq!(changed(DEFAULT, 4), 48);
        assert_eq!(changed(DEFAULT, -8), 36);
        assert_eq!(changed(26, -4), 24, "the narrowest");
        assert_eq!(changed(78, 12), 80, "the widest");
        assert_eq!(changed(30, isize::MIN), 24);
        assert_eq!(changed(30, isize::MAX), 80);
    }
}
