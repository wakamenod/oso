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
