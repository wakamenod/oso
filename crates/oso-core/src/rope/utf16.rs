//! Counting a rope's text in UTF-16 code units, for language servers that place positions
//! in them (ADR 0138). Each chunk's summary keeps its count, so a conversion takes O(log n)
//! on any line, as a byte column does.
//!
//! Must not depend on: `decode`, `display_map`, cells or display rows.

use super::{Rope, TextSummary};
use crate::sum_tree::{Bias, Dimension};

/// The UTF-16 code units of `text`.
fn units(text: &str) -> usize {
    let counts = super::count::counts(text);
    counts.chars + counts.four_byte_chars
}

/// A count of UTF-16 code units, to seek a rope by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Utf16(usize);

impl Dimension<TextSummary> for Utf16 {
    fn add_summary(&mut self, summary: &TextSummary) {
        self.0 += summary.utf16;
    }
}

impl Rope {
    /// The UTF-16 code units before `offset`, clipped to a character's start. O(log n).
    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let offset = self.clip_offset(offset, Bias::Left);
        let mut cursor = self.tree.cursor::<(usize, Utf16)>();
        cursor.seek(&offset, Bias::Right);
        let (start, units_before) = *cursor.start();
        let within = cursor
            .item()
            .map_or(0, |chunk| units(&chunk.as_str()[..offset - start]));
        units_before.0 + within
    }

    /// The offset after `units` UTF-16 code units from the start. A count inside a surrogate
    /// pair gives the start of its character, and a count past the end the end. O(log n).
    pub fn utf16_to_offset(&self, units: usize) -> usize {
        let mut cursor = self.tree.cursor::<(Utf16, usize)>();
        cursor.seek(&Utf16(units), Bias::Right);
        let (start, offset) = *cursor.start();
        let Some(chunk) = cursor.item() else {
            return self.len();
        };
        let mut left = units - start.0;
        for (index, character) in chunk.as_str().char_indices() {
            let width = character.len_utf16();
            if left < width {
                return offset + index;
            }
            left -= width;
        }
        offset + chunk.as_str().len()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{floor_boundary, text_strategy};
    use super::*;
    use proptest::prelude::*;

    /// The offset after `units` code units by a naive scan, as `utf16_to_offset` defines it.
    fn naive_offset(text: &str, units: usize) -> usize {
        let mut left = units;
        for (index, character) in text.char_indices() {
            if left < character.len_utf16() {
                return index;
            }
            left -= character.len_utf16();
        }
        text.len()
    }

    #[test]
    fn units_count_surrogate_pairs_twice() {
        assert_eq!(units(""), 0);
        assert_eq!(units("a"), 1);
        assert_eq!(units("é日"), 2);
        assert_eq!(units("\u{1F600}"), 2);
        assert_eq!(units("a\u{10FFFF}\u{10000}b"), 6);
        assert_eq!(TextSummary::from_text("👨\u{200d}👩").utf16, 5);
    }

    #[test]
    fn a_count_inside_a_pair_gives_its_character_start() {
        let rope = Rope::from("a\u{1F600}b");
        assert_eq!(rope.utf16_to_offset(1), 1);
        assert_eq!(rope.utf16_to_offset(2), 1);
        assert_eq!(rope.utf16_to_offset(3), 5);
        assert_eq!(rope.utf16_to_offset(4), 6);
        assert_eq!(rope.utf16_to_offset(usize::MAX), 6);
        // Inside a UTF-8 character, its start.
        assert_eq!(rope.offset_to_utf16(3), 1);
        assert_eq!(rope.offset_to_utf16(usize::MAX), 4);
    }

    #[test]
    fn long_lines_convert_across_chunks() {
        let text = "x\u{1F600}日".repeat(2000);
        let rope = Rope::from(text.as_str());
        assert_eq!(rope.summary().utf16, 8000);
        for (offset, _) in text.char_indices().step_by(7) {
            let counted = units(&text[..offset]);
            assert_eq!(rope.offset_to_utf16(offset), counted);
            assert_eq!(rope.utf16_to_offset(counted), offset);
        }
    }

    proptest! {
        #[test]
        fn conversions_match_naive_scans(
            text in text_strategy(80),
            offset in 0usize..500,
            count in 0usize..300,
        ) {
            let rope = Rope::from(text.as_str());
            prop_assert_eq!(rope.summary().utf16, text.encode_utf16().count());
            let clipped = floor_boundary(&text, offset);
            let before = text[..clipped].encode_utf16().count();
            prop_assert_eq!(rope.offset_to_utf16(offset), before);
            prop_assert_eq!(rope.utf16_to_offset(before), clipped);
            prop_assert_eq!(rope.utf16_to_offset(count), naive_offset(&text, count));
        }

        #[test]
        fn edits_keep_the_count(
            text in text_strategy(60),
            edits in prop::collection::vec((0usize..400, 0usize..400, text_strategy(12)), 1..8),
        ) {
            let mut rope = Rope::from(text.as_str());
            let mut model = text;
            for (a, b, insert) in edits {
                let start = floor_boundary(&model, a.min(b));
                let end = floor_boundary(&model, a.max(b));
                rope.replace(start..end, &insert);
                model.replace_range(start..end, &insert);
                prop_assert_eq!(rope.summary().utf16, model.encode_utf16().count());
            }
        }
    }
}
