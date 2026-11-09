//! The tests of [`WrapSnapshot`](super::WrapSnapshot): its rows, their conversions, and
//! that an edit rewraps as a wrap from scratch would.
//!
//! Must not depend on: windows or the renderer.

use proptest::prelude::*;

use super::*;
use crate::display_map::{DisplayMap, TabWidth};
use crate::rope::Rope;

fn display(text: &str) -> DisplaySnapshot {
    DisplayMap::new(&Rope::from(text), TabWidth::new(4)).snapshot()
}

/// Each view row's text, with `|` after one that continues.
fn rows(wrap: &WrapSnapshot, display: &DisplaySnapshot, text: &str) -> Vec<String> {
    (0..wrap.row_count())
        .map(|index| {
            let row = wrap.row(display, index);
            let mark = if row.continues { "|" } else { "" };
            format!("{}{mark}", &text[row.start..row.end])
        })
        .collect()
}

/// Every row's start and whether it continues, to compare two wraps.
fn shape(wrap: &WrapSnapshot, display: &DisplaySnapshot) -> Vec<(usize, usize, bool)> {
    (0..wrap.row_count())
        .map(|index| {
            let row = wrap.row(display, index);
            (row.start, row.line, row.continues)
        })
        .collect()
}

#[test]
fn a_paragraph_wraps_at_the_width_less_the_marks_column() {
    let text = "one two three four\nshort\n";
    let display = display(text);
    let wrap = WrapSnapshot::new(&display, 9);
    assert_eq!(
        rows(&wrap, &display, text),
        ["one two |", "three |", "four\n", "short\n", ""]
    );
    assert!(wrap.wraps());
}

#[test]
fn a_window_too_narrow_does_not_wrap() {
    let text = "one two three four";
    let display = display(text);
    let wrap = WrapSnapshot::new(&display, MIN_COLUMNS - 1);
    assert_eq!(rows(&wrap, &display, text), [text]);
    assert!(!wrap.wraps());
}

#[test]
fn unwrapped_rows_are_the_logical_lines() {
    let text = "a\n\nlong line here\n";
    let display = display(text);
    let wrap = WrapSnapshot::unwrapped(&display);
    assert_eq!(
        rows(&wrap, &display, text),
        ["a\n", "\n", "long line here\n", ""]
    );
}

#[test]
fn an_empty_buffer_has_one_row() {
    let display = display("");
    let wrap = WrapSnapshot::new(&display, 20);
    assert_eq!(wrap.row_count(), 1);
    assert_eq!(wrap.row_of(&display, 0), 0);
    assert_eq!(wrap.row(&display, 0).start, 0);
}

#[test]
fn a_long_line_spans_many_items() {
    let text = "word ".repeat(2000);
    let display = display(&text);
    let wrap = WrapSnapshot::new(&display, 11);
    assert_eq!(wrap.row_count(), 1000);
    let row = wrap.row(&display, 700);
    assert_eq!((row.start, row.column, row.line), (7000, 7000, 0));
    assert_eq!(wrap.row_of(&display, 7004), 700);
}

#[test]
fn a_tab_across_the_edge_goes_to_the_next_row_with_its_width() {
    let text = "abcdefg\thi";
    let display = display(text);
    let wrap = WrapSnapshot::new(&display, 8);
    assert_eq!(rows(&wrap, &display, text), ["abcdefg|", "\thi"]);
    // It keeps the one cell it has at column 7 of its line.
    let row = wrap.row(&display, 1);
    assert_eq!(row.column, 7);
}

#[test]
fn debug_prints_no_offsets() {
    let display = display("one two three four");
    let wrap = WrapSnapshot::new(&display, 9);
    assert_eq!(format!("{wrap:?}"), "WrapSnapshot { rows: 3, .. }");
    assert_eq!(
        format!("{:?}", wrap.row(&display, 1)),
        "ViewRow { continues: true, .. }"
    );
}

/// Text for wrapping: words, CJK, kinsoku's punctuation, no-break spaces, tabs and the rope's
/// atoms.
fn wrap_text(max_atoms: usize) -> impl Strategy<Value = String> {
    let pieces = prop_oneof![
        prop::sample::select(crate::rope::tests::ATOMS),
        prop::sample::select(
            &[
                "word",
                "ab",
                " ",
                "  ",
                "日本",
                "語",
                "。",
                "、",
                "「",
                "」",
                "ー",
                "っ",
                "\u{a0}",
                ",",
                "(",
                "abcdefghijkl",
                "\n",
            ][..]
        ),
    ];
    prop::collection::vec(pieces, 0..max_atoms).prop_map(|pieces| pieces.concat())
}

proptest! {
    /// Each view row starts where the one before it ends, and `row_of` finds it again.
    #[test]
    fn rows_cover_the_text_and_convert_both_ways(
        text in wrap_text(60),
        columns in 8_usize..24,
    ) {
        let display = display(&text);
        let wrap = WrapSnapshot::new(&display, columns);
        let mut at = 0;
        for index in 0..wrap.row_count() {
            let row = wrap.row(&display, index);
            prop_assert_eq!(row.start, at);
            prop_assert!(row.end >= row.start);
            if row.end > row.start || index + 1 == wrap.row_count() {
                prop_assert_eq!(wrap.row_of(&display, row.start), index);
            }
            at = row.end;
        }
        prop_assert_eq!(at, text.len());
    }

    /// After any edit, the rows are those a wrap of the edited text from scratch gives.
    #[test]
    fn an_edit_rewraps_as_a_wrap_from_scratch(
        text in wrap_text(60),
        insert in wrap_text(6),
        start in 0_usize..400,
        len in 0_usize..12,
        columns in 8_usize..24,
        wrapped in any::<bool>(),
    ) {
        let mut rope = Rope::from(text.as_str());
        let mut map = DisplayMap::new(&rope, TabWidth::new(4));
        let before = map.snapshot();
        let wrap = if wrapped {
            WrapSnapshot::new(&before, columns)
        } else {
            WrapSnapshot::unwrapped(&before)
        };
        let start = crate::rope::tests::floor_boundary(&text, start);
        let end = crate::rope::tests::floor_boundary(&text, start + len);
        let edit = rope.replace(start..end, &insert);
        map.sync(&rope, &edit);
        let after = map.snapshot();
        let edited = wrap.edited(&before, &after, &edit);
        let fresh = if wrapped {
            WrapSnapshot::new(&after, columns)
        } else {
            WrapSnapshot::unwrapped(&after)
        };
        prop_assert_eq!(shape(&edited, &after), shape(&fresh, &after));
        prop_assert_eq!(edited.tree.summary(), fresh.tree.summary());
    }
}

proptest! {
    /// Edits one after another, as typing makes them, keep the rows a wrap from scratch
    /// gives.
    #[test]
    fn edits_in_a_row_rewrap_as_a_wrap_from_scratch(
        text in wrap_text(40),
        edits in prop::collection::vec((0_usize..300, 0_usize..6, wrap_text(3)), 1..8),
        columns in 8_usize..20,
    ) {
        let mut rope = Rope::from(text.as_str());
        let mut map = DisplayMap::new(&rope, TabWidth::new(4));
        let mut wrap = WrapSnapshot::new(&map.snapshot(), columns);
        for (start, len, insert) in edits {
            let now = rope.to_string();
            let start = crate::rope::tests::floor_boundary(&now, start);
            let end = crate::rope::tests::floor_boundary(&now, start + len);
            let edit = rope.replace(start..end, &insert);
            let before = map.snapshot();
            map.sync(&rope, &edit);
            wrap = wrap.edited(&before, &map.snapshot(), &edit);
        }
        let after = map.snapshot();
        let fresh = WrapSnapshot::new(&after, columns);
        prop_assert_eq!(shape(&wrap, &after), shape(&fresh, &after));
    }
}

proptest! {
    /// Rows cover a text with invalid bytes, which show as `\\xNN` and make it read-only,
    /// so that it is wrapped but never edited.
    #[test]
    fn rows_cover_a_text_with_invalid_bytes(
        pieces in prop::collection::vec(
            prop_oneof![wrap_text(3).prop_map(String::into_bytes), Just(b"\xff".to_vec()), Just(b"\xc3".to_vec())],
            0..30,
        ),
        columns in 8_usize..24,
        tab_width in 1_u32..13,
    ) {
        // As a load does: each invalid byte a U+FFFD, noted before the text that holds it.
        let mut text = String::new();
        let mut invalid = Vec::new();
        for piece in &pieces {
            match std::str::from_utf8(piece) {
                Ok(valid) => text.push_str(valid),
                Err(_) => {
                    invalid.push((text.len(), piece[0]));
                    text.push('\u{fffd}');
                }
            }
        }
        let mut rope = Rope::new();
        let mut map = DisplayMap::new(&rope, TabWidth::new(tab_width));
        map.extend_invalid(0, &invalid);
        let edit = rope.push_str(&text);
        map.sync(&rope, &edit);
        let display = map.snapshot();
        let wrap = WrapSnapshot::new(&display, columns);
        let mut at = 0;
        for index in 0..wrap.row_count() {
            let row = wrap.row(&display, index);
            prop_assert_eq!(row.start, at);
            at = row.end;
        }
        prop_assert_eq!(at, text.len());
    }

    /// Edits keep the rows a wrap from scratch gives at any tab width.
    #[test]
    fn edits_at_any_tab_width_rewrap_as_a_wrap_from_scratch(
        text in wrap_text(50),
        insert in wrap_text(4),
        start in 0_usize..300,
        len in 0_usize..10,
        columns in 8_usize..24,
        tab_width in 1_u32..13,
    ) {
        let mut rope = Rope::from(text.as_str());
        let mut map = DisplayMap::new(&rope, TabWidth::new(tab_width));
        let before = map.snapshot();
        let wrap = WrapSnapshot::new(&before, columns);
        let start = crate::rope::tests::floor_boundary(&text, start);
        let end = crate::rope::tests::floor_boundary(&text, start + len);
        let edit = rope.replace(start..end, &insert);
        map.sync(&rope, &edit);
        let after = map.snapshot();
        let edited = wrap.edited(&before, &after, &edit);
        let fresh = WrapSnapshot::new(&after, columns);
        prop_assert_eq!(shape(&edited, &after), shape(&fresh, &after));
    }

    /// Several splices of one command, applied from the end back as the editor applies them,
    /// keep the rows a wrap from scratch gives, with no budget to run out of.
    #[test]
    fn a_command_of_several_splices_rewraps_as_a_wrap_from_scratch(
        text in wrap_text(50),
        places in prop::collection::vec((0_usize..300, 0_usize..5, wrap_text(2)), 1..6),
        columns in 8_usize..20,
    ) {
        let mut rope = Rope::from(text.as_str());
        let mut map = DisplayMap::new(&rope, TabWidth::new(4));
        let wrap = WrapSnapshot::new(&map.snapshot(), columns);
        let mut places = places;
        places.sort_by_key(|&(start, ..)| std::cmp::Reverse(start));
        let mut splices = Vec::new();
        for (start, len, insert) in places {
            let now = rope.to_string();
            let start = crate::rope::tests::floor_boundary(&now, start);
            let end = crate::rope::tests::floor_boundary(&now, start + len);
            let edit = rope.replace(start..end, &insert);
            let before = map.snapshot();
            map.sync(&rope, &edit);
            splices.push(Splice { before, after: map.snapshot(), edit });
        }
        let after = map.snapshot();
        let mut unlimited = usize::MAX;
        let (edited, stale) = wrap.edited_command(&splices, &mut unlimited);
        prop_assert!(stale.is_empty());
        prop_assert_eq!(shape(&edited, &after), shape(&WrapSnapshot::new(&after, columns), &after));
    }
}

proptest! {
    /// A line found to fit from its bytes has the row a scan of its clusters gives it.
    #[test]
    fn a_plain_line_fits_as_its_clusters_say(
        text in prop::collection::vec(
            prop::sample::select(&["a", " ", "\t", "\r", "\r\n", "\n", "~", "\u{7f}", "é", "日"][..]),
            0..40,
        ),
        columns in 8_usize..16,
        tab_width in 1_u32..9,
    ) {
        let text = text.concat();
        let display = DisplayMap::new(&Rope::from(text.as_str()), TabWidth::new(tab_width)).snapshot();
        let plain = WrapSnapshot::new(&display, columns);
        let scanned = WrapSnapshot::scanned(&display, columns);
        prop_assert_eq!(shape(&plain, &display), shape(&scanned, &display));
    }
}

/// One edit with the budget already spent, against a wrap from scratch.
fn spent_edit(text: &str, (start, end, insert): (usize, usize, &str), columns: usize) {
    let mut rope = Rope::from(text);
    let mut map = DisplayMap::new(&rope, TabWidth::new(4));
    let before = map.snapshot();
    let wrap = WrapSnapshot::new(&before, columns);
    let edit = rope.replace(start..end, insert);
    map.sync(&rope, &edit);
    let after = map.snapshot();
    let (edited, _) = wrap.edited_within(&before, &after, &edit, &mut 0);
    let mut at = 0;
    for index in 0..edited.row_count() {
        let row = edited.row(&after, index);
        assert_eq!(row.start, at, "rows tile");
        at = row.end;
    }
    assert_eq!(at, after.len());
    let lines = (0..edited.row_count())
        .filter(|&index| !edited.row(&after, index).continues)
        .count();
    assert_eq!(lines, after.row_count(), "a row ends each line");
}

#[test]
fn a_spent_edit_at_the_end_keeps_the_empty_last_line() {
    spent_edit("abc", (3, 3, "\n"), 9);
    spent_edit(&"word ".repeat(10), (50, 50, "\n"), 9);
}

#[test]
fn a_spent_empty_edit_inside_a_line_break_keeps_the_rows_tiled() {
    spent_edit("ab\n\r\n", (4, 4, ""), 9);
}
