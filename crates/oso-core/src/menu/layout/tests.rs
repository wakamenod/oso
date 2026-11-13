//! Tests of where a menu's float and its parts go (ADR 0117): sizes, rows of columns, and what
//! a small frame gives up.
//!
//! Must not depend on: `keymap`; layouts are tested with plain sizes.

use proptest::prelude::*;

use super::{Layout, layout};
use crate::command::{ArgSpec, Args, Command, CommandError, CommandId, Ctx, Reach, Registry};
use crate::frame::CellRect;
use crate::menu::{Definition, Head, HeadAction, Note, Notes};

fn noop(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    Ok(())
}

fn command() -> CommandId {
    let mut registry = Registry::new();
    registry
        .register(Command {
            name: "noop",
            doc: "Does nothing.",
            args: ArgSpec::None,
            reach: Reach::Look,
            run: noop,
            state: None,
        })
        .unwrap()
}

fn area(rows: usize, columns: usize) -> CellRect {
    CellRect {
        row: 0,
        column: 0,
        rows,
        columns,
    }
}

/// A group: its title, its row, and its heads' keys and labels.
type Group<'g> = (Option<&'g str>, u8, &'g [(&'g str, &'g str)]);

/// A menu of `groups`.
fn menu(groups: &[Group<'_>]) -> Definition {
    let id = command();
    let mut menu = Definition::new("windows", "Windows").unwrap();
    for (title, row, heads) in groups {
        menu.add_group(*title, *row).unwrap();
        for (keys, label) in *heads {
            menu.add_head(Head {
                keys: keys.parse().unwrap(),
                label: (*label).to_owned(),
                action: HeadAction::Command(id),
                stay: None,
                count: None,
            })
            .unwrap();
        }
    }
    menu
}

fn size_menu() -> Definition {
    menu(&[(Some("Size"), 1, &[("a", "narrower"), ("b", "wider")])])
}

fn rect(row: usize, column: usize, rows: usize, columns: usize) -> CellRect {
    CellRect {
        row,
        column,
        rows,
        columns,
    }
}

#[test]
fn a_menu_is_centred_and_fitted_to_its_content() {
    let layout = layout(&size_menu(), &Notes::default(), &[], 0, area(40, 100));
    // `a  narrower` is 11 cells; with 2 columns and 1 row of padding on each side: 15 by 10, for
    // the title, a blank row, the column's title and rule, two heads, a blank row and `q quit`.
    assert_eq!(layout.outer, rect(15, 42, 10, 15));
    assert_eq!(layout.inner, rect(16, 44, 8, 11));
    assert_eq!(layout.title, rect(16, 44, 1, 11));
    assert_eq!(layout.line, None);
    let column = &layout.columns[0];
    assert_eq!(column.title, Some(rect(18, 44, 1, 11)));
    assert_eq!(column.rule, Some(rect(19, 44, 1, 11)));
    let cells: Vec<_> = column
        .rows
        .iter()
        .map(|row| (row.head, row.cells))
        .collect();
    assert_eq!(cells, [(0, rect(20, 44, 1, 11)), (1, rect(21, 44, 1, 11))]);
    // `a`, two cells apart, then the label.
    let row = &column.rows[0];
    assert_eq!(
        (row.key, row.label),
        (rect(20, 44, 1, 1), rect(20, 47, 1, 8))
    );
    assert_eq!((row.check, row.note), (None, None));
    assert_eq!(column.more, None);
    assert_eq!(layout.quit, Some(rect(23, 44, 1, 6)));
}

#[test]
fn checks_notes_and_the_line_widen_it() {
    let notes = Notes {
        line: Some("session one, project oso".to_owned()),
        heads: vec![(
            1,
            Note {
                text: "12 files".to_owned(),
                ..Note::default()
            },
        )],
        ..Notes::default()
    };
    let checks = [Some(true), None];
    let layout = layout(&size_menu(), &notes, &checks, 0, area(40, 100));
    // The labels start in one column, after `a` and 2 cells, and the notes in another, after
    // the widest `[x] narrower` and 2 cells: `b  wider         12 files` is 25 cells, more than
    // the line's 24.
    assert_eq!(layout.inner.columns, 25);
    let (x, y) = (layout.inner.column, layout.columns[0].rows[0].cells.row);
    let [narrower, wider] = &layout.columns[0].rows[..] else {
        panic!("two rows");
    };
    assert_eq!(narrower.check, Some(rect(y, x + 3, 1, 4)));
    assert_eq!(narrower.label, rect(y, x + 7, 1, 8));
    assert_eq!(narrower.note, None);
    assert_eq!(wider.check, None);
    assert_eq!(wider.label, rect(y + 1, x + 3, 1, 5));
    assert_eq!(wider.note, Some(rect(y + 1, x + 17, 1, 8)));
    assert_eq!(
        layout.line,
        Some(rect(layout.inner.row + 1, layout.inner.column, 1, 25))
    );
    // The columns start a row lower for the line.
    assert_eq!(layout.columns[0].title.unwrap().row, layout.inner.row + 3);
}

#[test]
fn hidden_heads_and_groups_left_with_none_are_left_out() {
    let menu = menu(&[
        (Some("One"), 1, &[("a", "first"), ("b", "second")]),
        (Some("Two"), 1, &[("c", "third")]),
    ]);
    let hidden = Note {
        hidden: true,
        ..Note::default()
    };
    let notes = Notes {
        heads: vec![(0, hidden.clone()), (2, hidden)],
        ..Notes::default()
    };
    let layout = layout(&menu, &notes, &[], 0, area(40, 100));
    assert_eq!(layout.columns.len(), 1);
    let rows: Vec<usize> = layout.columns[0].rows.iter().map(|row| row.head).collect();
    assert_eq!(rows, [1]);
}

#[test]
fn groups_sit_side_by_side_in_their_rows() {
    let menu = menu(&[
        (Some("Size"), 1, &[("a", "narrower"), ("b", "wider")]),
        (Some("Split"), 1, &[("-", "below")]),
        (Some("Buffer"), 2, &[(",", "previous")]),
    ]);
    let layout = layout(&menu, &Notes::default(), &[], 0, area(40, 100));
    let [size, split, buffer] = &layout.columns[..] else {
        panic!("three columns");
    };
    let at = |column: &crate::menu::Column| column.title.unwrap();
    // Three columns apart.
    assert_eq!(at(split).column, at(size).column + 11 + 3);
    assert_eq!(at(split).row, at(size).row);
    // The second row starts below the tallest column of the first, past a blank row.
    assert_eq!(at(buffer).column, at(size).column);
    assert_eq!(at(buffer).row, at(size).row + 4 + 1);
}

#[test]
fn q_is_not_drawn_where_a_head_binds_it() {
    let menu = menu(&[(None, 1, &[("q", "quote")])]);
    let layout = layout(&menu, &Notes::default(), &[], 0, area(40, 100));
    assert_eq!(layout.quit, None);
    assert_eq!(layout.columns[0].title, None);
}

#[test]
fn a_small_frame_gives_up_padding_then_titles() {
    let menu = size_menu();
    let notes = Notes::default();
    // Padded, it takes 10 rows, which a 12-row area leaves room for.
    assert_ne!(
        layout(&menu, &notes, &[], 0, area(12, 100)).outer,
        layout(&menu, &notes, &[], 0, area(12, 100)).inner
    );
    // Unpadded: the title, the column's title and rule, two heads and `q quit`: 6 rows.
    let unpadded = layout(&menu, &notes, &[], 0, area(8, 100));
    assert_eq!(unpadded.outer, unpadded.inner);
    assert_eq!(unpadded.inner.rows, 6);
    assert!(unpadded.columns[0].title.is_some());
    // Without the column's title: 4 rows.
    let untitled = layout(&menu, &notes, &[], 0, area(6, 100));
    assert_eq!(untitled.inner.rows, 4);
    assert_eq!(untitled.columns[0].title, None);
    assert_eq!(untitled.columns[0].rows.len(), 2);
}

#[test]
fn a_narrow_frame_moves_columns_onto_a_new_row() {
    let menu = menu(&[
        (Some("One"), 1, &[("a", "first one")]),
        (Some("Two"), 1, &[("b", "second one")]),
    ]);
    let wide = layout(&menu, &Notes::default(), &[], 0, area(40, 100));
    assert_eq!(
        wide.columns[0].rows[0].cells.row,
        wide.columns[1].rows[0].cells.row
    );
    // `a  first one` and `b  second one` take 12 and 13 cells, 28 with the gap: a room of 20
    // columns holds one.
    let narrow = layout(&menu, &Notes::default(), &[], 0, area(40, 24));
    let (one, two) = (&narrow.columns[0], &narrow.columns[1]);
    assert_eq!(one.rows[0].cells.column, two.rows[0].cells.column);
    assert!(two.rows[0].cells.row > one.rows[0].cells.row);
}

#[test]
fn a_short_frame_cuts_the_longest_columns() {
    let heads: Vec<(String, String)> = (0..10)
        .map(|i| (char::from(b'a' + i).to_string(), format!("head {i}")))
        .collect();
    let heads: Vec<(&str, &str)> = heads
        .iter()
        .map(|(k, l)| (k.as_str(), l.as_str()))
        .collect();
    let menu = menu(&[(None, 1, &heads), (None, 1, &[("z", "short")])]);
    // 8 rows: the title and `q quit` leave 6 for the columns.
    let layout = layout(&menu, &Notes::default(), &[], 0, area(10, 100));
    assert_eq!(layout.inner.rows, 8);
    let long = &layout.columns[0];
    assert_eq!(long.rows.len(), 5);
    assert_eq!(long.more.map(|(n, _)| n), Some(5));
    assert_eq!(layout.columns[1].rows.len(), 1);
    assert_eq!(layout.columns[1].more, None);
}

#[test]
fn the_prefix_argument_ends_the_title_row_and_may_widen_it() {
    // `Windows  C-u 5-` is 15 cells, wider than the column's 11.
    let layout = layout(&size_menu(), &Notes::default(), &[], 6, area(40, 100));
    assert_eq!(layout.inner.columns, 15);
    let row = layout.inner.row;
    let end = layout.inner.column + 15;
    assert_eq!(layout.prefix, Some(rect(row, end - 6, 1, 6)));
    assert_eq!(layout.title, rect(row, layout.inner.column, 1, 7));
    // With none, the title has the row.
    let none = super::layout(&size_menu(), &Notes::default(), &[], 0, area(40, 100));
    assert_eq!((none.prefix, none.title.columns), (None, 11));
}

#[test]
fn a_cut_column_says_how_many_on_the_row_below_its_last() {
    let heads = many("x", 10);
    let menu = menu(&[(None, 1, &borrowed(&heads))]);
    let layout = layout(&menu, &Notes::default(), &[], 0, area(8, 100));
    let column = &layout.columns[0];
    let last = column.rows.last().unwrap().cells;
    let (cut, at) = column.more.unwrap();
    assert_eq!(cut + column.rows.len(), 10);
    // `… 7 more` is 8 cells.
    assert_eq!(at, rect(last.row + 1, last.column, 1, 8));
}

#[test]
fn a_long_note_is_measured_only_to_its_cap() {
    let notes = Notes {
        heads: vec![(
            0,
            Note {
                text: "n".repeat(100_000),
                ..Note::default()
            },
        )],
        ..Notes::default()
    };
    let layout = layout(&size_menu(), &notes, &[], 0, area(40, 1000));
    let note = layout.columns[0].rows[0].note.unwrap();
    assert_eq!(note.columns, 512);
    assert_eq!(super::capped("ab\u{3042}"), "ab\u{3042}");
    let long = "\u{3042}".repeat(200);
    assert_eq!(
        super::capped(&long).len(),
        510,
        "cut at a character's boundary"
    );
}

/// `count` heads of keys `prefix a`, `prefix b`, …, labelled `x`.
fn many(prefix: &str, count: u8) -> Vec<(String, String)> {
    (0..count)
        .map(|i| (format!("{prefix} {}", char::from(b'a' + i)), "x".to_owned()))
        .collect()
}

fn borrowed(heads: &[(String, String)]) -> Vec<(&str, &str)> {
    heads
        .iter()
        .map(|(k, l)| (k.as_str(), l.as_str()))
        .collect()
}

#[test]
fn a_column_cut_to_one_row_counts_all_its_heads() {
    let heads = many("x", 10);
    let menu = menu(&[(None, 1, &borrowed(&heads))]);
    // 3 rows: the title, `… 10 more` and `q quit`.
    let layout = layout(&menu, &Notes::default(), &[], 0, area(5, 100));
    assert_eq!(layout.inner.rows, 3);
    let column = &layout.columns[0];
    assert!(column.rows.is_empty());
    assert_eq!(column.more.map(|(n, _)| n), Some(10));
    // `x a  x` is 6 cells, `Windows` 7: the column is as wide as `… 10 more`, 9 cells.
    assert_eq!(layout.inner.columns, 9);
}

#[test]
fn columns_are_cut_a_row_at_a_time_the_longest_first() {
    let (long, short) = (many("x", 20), many("y", 10));
    let menu = menu(&[(None, 1, &borrowed(&long)), (None, 2, &borrowed(&short))]);
    // 17 rows: the title and `q quit` leave 15 for the two rows of columns, which take 30. The
    // long one is cut to 10 first, then each in turn, the first of two as long first: 7 and 8,
    // not 7 and 7 under one limit for both.
    let layout = layout(&menu, &Notes::default(), &[], 0, area(19, 100));
    assert_eq!(layout.inner.rows, 17);
    let (long, short) = (&layout.columns[0], &layout.columns[1]);
    assert_eq!((long.rows.len(), long.more.map_or(0, |(n, _)| n)), (6, 14));
    assert_eq!((short.rows.len(), short.more.map_or(0, |(n, _)| n)), (7, 3));
    // Where the long one alone is too tall, only it is cut.
    let layout = super::layout(&menu, &Notes::default(), &[], 0, area(26, 100));
    let (long, short) = (&layout.columns[0], &layout.columns[1]);
    assert_eq!((long.rows.len(), long.more.map_or(0, |(n, _)| n)), (11, 9));
    assert_eq!(
        (short.rows.len(), short.more.map_or(0, |(n, _)| n)),
        (10, 0)
    );
}

/// Every part of `layout` is inside `inner`, and no two rows overlap.
fn well_placed(layout: &Layout) -> Result<(), TestCaseError> {
    let inner = layout.inner;
    let inside = |r: &CellRect| {
        r.rows == 0
            || (r.row >= inner.row
                && r.row + r.rows <= inner.row + inner.rows
                && r.column >= inner.column
                && r.column + r.columns <= inner.column + inner.columns)
    };
    let mut rows: Vec<CellRect> = layout
        .columns
        .iter()
        .flat_map(|c| c.rows.iter().map(|r| r.cells))
        .collect();
    for row in layout.columns.iter().flat_map(|c| &c.rows) {
        let parts = [Some(row.key), row.check, Some(row.label), row.note];
        for part in parts.into_iter().flatten() {
            let within = part.row == row.cells.row
                && part.column >= row.cells.column
                && part.column + part.columns <= row.cells.column + row.cells.columns;
            prop_assert!(within, "{part:?} outside its row {:?}", row.cells);
        }
    }
    rows.extend(layout.columns.iter().filter_map(|c| c.title));
    rows.extend(layout.columns.iter().filter_map(|c| c.rule));
    rows.extend(layout.columns.iter().filter_map(|c| c.more.map(|(_, r)| r)));
    rows.extend(layout.prefix);
    rows.extend(layout.quit);
    rows.extend(layout.line);
    for r in &rows {
        prop_assert!(inside(r), "{r:?} outside {inner:?}");
    }
    prop_assert!(inside(&layout.title));
    if let Some(prefix) = layout.prefix {
        prop_assert!(layout.title.column + layout.title.columns <= prefix.column);
    }
    for (i, a) in rows.iter().enumerate() {
        for b in &rows[i + 1..] {
            let apart = a.row != b.row
                || a.column + a.columns <= b.column
                || b.column + b.columns <= a.column;
            prop_assert!(apart, "{a:?} overlaps {b:?}");
        }
    }
    Ok(())
}

proptest! {
    #[test]
    fn prop_a_menu_stays_in_its_area(
        groups in prop::collection::vec((1u8..=3, 1usize..12, 1usize..20), 1..8),
        rows in 0usize..60,
        columns in 0usize..200,
        prefix in 0usize..12,
    ) {
        let labels: Vec<String> = (0..20).map(|n| "x".repeat(n)).collect();
        let mut menu = Definition::new("m", "Title").unwrap();
        let id = command();
        let mut n = 0u32;
        for (row, count, label) in groups {
            if menu.add_group(Some("Group"), row).is_err() {
                continue;
            }
            for _ in 0..count {
                let key = format!("<f{}> <f{}>", n / 12 + 1, n % 12 + 1);
                n += 1;
                let _ = menu.add_head(Head {
                    keys: key.parse().unwrap(),
                    label: labels[label].clone(),
                    action: HeadAction::Command(id),
                    stay: None,
                    count: None,
                });
            }
        }
        let area = area(rows, columns);
        let layout = layout(&menu, &Notes::default(), &[], prefix, area);
        prop_assert!(layout.outer.row + layout.outer.rows <= area.rows);
        prop_assert!(layout.outer.column + layout.outer.columns <= area.columns);
        if rows >= 2 && columns >= 4 {
            prop_assert!(layout.outer.row >= 1 && layout.outer.column >= 2);
            prop_assert!(layout.outer.row + layout.outer.rows < area.rows);
        }
        well_placed(&layout)?;
        // Every head is shown or counted, in a frame tall enough for one row of each column.
        if rows >= 40 && columns >= 200 {
            let shown: usize = layout.columns.iter().map(|c| c.rows.len() + c.more.map_or(0, |(n, _)| n)).sum();
            prop_assert_eq!(shown, menu.heads().len());
        }
    }
}
