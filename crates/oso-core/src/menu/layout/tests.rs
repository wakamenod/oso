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
    let layout = layout(&size_menu(), &Notes::default(), &[], area(40, 100));
    // `a  narrower` is 11 cells; with 2 columns and 1 row of padding on each side: 15 by 10, for
    // the title, a blank row, the column's title and rule, two heads, a blank row and `q quit`.
    assert_eq!(layout.outer, rect(15, 42, 10, 15));
    assert_eq!(layout.inner, rect(16, 44, 8, 11));
    assert_eq!(layout.title, rect(16, 44, 1, 11));
    assert_eq!(layout.line, None);
    let column = &layout.columns[0];
    assert_eq!(column.title, Some(rect(18, 44, 1, 11)));
    assert_eq!(column.keys, 1);
    assert_eq!(
        column.rows,
        [(0, rect(20, 44, 1, 11)), (1, rect(21, 44, 1, 11))]
    );
    assert_eq!(column.more, 0);
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
    let layout = layout(&size_menu(), &notes, &checks, area(40, 100));
    // `a  [x] narrower` is 15 cells and `b  wider  12 files` 18; the line is 24.
    assert_eq!(layout.inner.columns, 24);
    assert_eq!(layout.columns[0].rows[0].1.columns, 18);
    assert_eq!(
        layout.line,
        Some(rect(layout.inner.row + 1, layout.inner.column, 1, 24))
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
    let layout = layout(&menu, &notes, &[], area(40, 100));
    assert_eq!(layout.columns.len(), 1);
    let rows: Vec<usize> = layout.columns[0]
        .rows
        .iter()
        .map(|(head, _)| *head)
        .collect();
    assert_eq!(rows, [1]);
}

#[test]
fn groups_sit_side_by_side_in_their_rows() {
    let menu = menu(&[
        (Some("Size"), 1, &[("a", "narrower"), ("b", "wider")]),
        (Some("Split"), 1, &[("-", "below")]),
        (Some("Buffer"), 2, &[(",", "previous")]),
    ]);
    let layout = layout(&menu, &Notes::default(), &[], area(40, 100));
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
    let layout = layout(&menu, &Notes::default(), &[], area(40, 100));
    assert_eq!(layout.quit, None);
    assert_eq!(layout.columns[0].title, None);
}

#[test]
fn a_small_frame_gives_up_padding_then_titles() {
    let menu = size_menu();
    let notes = Notes::default();
    // Padded, it takes 10 rows, which a 12-row area leaves room for.
    assert_ne!(
        layout(&menu, &notes, &[], area(12, 100)).outer,
        layout(&menu, &notes, &[], area(12, 100)).inner
    );
    // Unpadded: the title, the column's title and rule, two heads and `q quit`: 6 rows.
    let unpadded = layout(&menu, &notes, &[], area(8, 100));
    assert_eq!(unpadded.outer, unpadded.inner);
    assert_eq!(unpadded.inner.rows, 6);
    assert!(unpadded.columns[0].title.is_some());
    // Without the column's title: 4 rows.
    let untitled = layout(&menu, &notes, &[], area(6, 100));
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
    let wide = layout(&menu, &Notes::default(), &[], area(40, 100));
    assert_eq!(wide.columns[0].rows[0].1.row, wide.columns[1].rows[0].1.row);
    // `a  first one` and `b  second one` take 12 and 13 cells, 28 with the gap: a room of 20
    // columns holds one.
    let narrow = layout(&menu, &Notes::default(), &[], area(40, 24));
    let (one, two) = (&narrow.columns[0], &narrow.columns[1]);
    assert_eq!(one.rows[0].1.column, two.rows[0].1.column);
    assert!(two.rows[0].1.row > one.rows[0].1.row);
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
    let layout = layout(&menu, &Notes::default(), &[], area(10, 100));
    assert_eq!(layout.inner.rows, 8);
    let long = &layout.columns[0];
    assert_eq!(long.rows.len(), 5);
    assert_eq!(long.more, 5);
    assert_eq!(layout.columns[1].rows.len(), 1);
    assert_eq!(layout.columns[1].more, 0);
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
    let layout = layout(&menu, &Notes::default(), &[], area(5, 100));
    assert_eq!(layout.inner.rows, 3);
    let column = &layout.columns[0];
    assert!(column.rows.is_empty());
    assert_eq!(column.more, 10);
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
    let layout = layout(&menu, &Notes::default(), &[], area(19, 100));
    assert_eq!(layout.inner.rows, 17);
    let (long, short) = (&layout.columns[0], &layout.columns[1]);
    assert_eq!((long.rows.len(), long.more), (6, 14));
    assert_eq!((short.rows.len(), short.more), (7, 3));
    // Where the long one alone is too tall, only it is cut.
    let layout = super::layout(&menu, &Notes::default(), &[], area(26, 100));
    let (long, short) = (&layout.columns[0], &layout.columns[1]);
    assert_eq!((long.rows.len(), long.more), (11, 9));
    assert_eq!((short.rows.len(), short.more), (10, 0));
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
        .flat_map(|c| c.rows.iter().map(|(_, r)| *r))
        .collect();
    rows.extend(layout.columns.iter().filter_map(|c| c.title));
    rows.extend(layout.quit);
    rows.extend(layout.line);
    for r in &rows {
        prop_assert!(inside(r), "{r:?} outside {inner:?}");
    }
    prop_assert!(inside(&layout.title));
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
        let layout = layout(&menu, &Notes::default(), &[], area);
        prop_assert!(layout.outer.row + layout.outer.rows <= area.rows);
        prop_assert!(layout.outer.column + layout.outer.columns <= area.columns);
        if rows >= 2 && columns >= 4 {
            prop_assert!(layout.outer.row >= 1 && layout.outer.column >= 2);
            prop_assert!(layout.outer.row + layout.outer.rows < area.rows);
        }
        well_placed(&layout)?;
        // Every head is shown or counted, in a frame tall enough for one row of each column.
        if rows >= 40 && columns >= 200 {
            let shown: usize = layout.columns.iter().map(|c| c.rows.len() + c.more).sum();
            prop_assert_eq!(shown, menu.heads().len());
        }
    }
}
