//! Tests of applying an undo group a slice at a time (ADR 0119): a splice's text put back in
//! pieces, and what the slices count.
//!
//! Must not depend on: `window`, `editor` or `command`; these see only text.

use super::super::splice::tests::{apply_in_slices, plain};
use super::*;
use crate::display_map::TabWidth;
use crate::display_map::tests::check_against;

#[test]
fn ranges_added_join_those_they_meet() {
    let mut ranges = Vec::new();
    add_descending(&mut ranges, 20..30);
    add_descending(&mut ranges, 5..8);
    assert_eq!(ranges, [20..30, 5..8]);
    // Out of order, between them, and touching the one below.
    add_descending(&mut ranges, 8..12);
    assert_eq!(ranges, [20..30, 5..12]);
    add_descending(&mut ranges, 40..41);
    assert_eq!(ranges, [40..41, 20..30, 5..12]);
    add_descending(&mut ranges, 0..50);
    assert_eq!(ranges.first(), Some(&(0..50)));
    assert_eq!(ranges.len(), 1);
}

#[test]
fn a_kill_goes_back_in_pieces_and_undoes_as_one_splice() {
    let killed = "日本語 text\r\n".repeat(40);
    let text = "before|after";
    let group: Vec<Batch> = vec![vec![Splice {
        start: 7,
        len: 0,
        text: Some(Rope::from(killed.as_str())),
    }]];
    let mut rope = Rope::from(text);
    let mut display = DisplayMap::new(&rope, TabWidth::new(4));
    let (steps, undo, _) =
        apply_in_slices(&group, &mut rope, &mut display, None, (SPLICE_WORK + 10, 7));
    assert_eq!(rope.to_string(), format!("before|{killed}after"));
    // Cursors follow the splice once, when its last piece is in.
    assert_eq!(steps.len(), 1);
    assert_eq!(
        steps[0],
        [OffsetEdit {
            start: 7,
            old_len: 0,
            new_len: killed.len()
        }]
    );
    assert_eq!(plain(&undo), [[(7, killed.len(), String::new())]]);
    check_against(&display.snapshot(), &rope.to_string(), TabWidth::new(4));
}

#[test]
fn the_work_done_grows_to_the_group_s_and_no_further() {
    let group: Vec<Batch> = (0..20)
        .map(|line| {
            vec![Splice {
                start: line * 10,
                len: 2,
                text: Some(Rope::from("abc")),
            }]
        })
        .collect();
    let counts: Vec<Counts> = group.iter().map(Counts::of).collect();
    let mut rope = Rope::from("x".repeat(300).as_str());
    let mut display = DisplayMap::new(&rope, TabWidth::new(4));
    let mut apply = GroupApply::default();
    let mut last = 0;
    let mut slices = 0;
    loop {
        let mut budget = Budget {
            work: SPLICE_WORK,
            until: None,
            piece: PIECE,
        };
        let ended = apply.go_on(
            &group,
            &counts,
            &mut rope,
            &mut display,
            None,
            &mut budget,
            &mut Vec::new(),
        );
        slices += 1;
        assert!(apply.done() > last, "each slice does some work");
        last = apply.done();
        if ended {
            break;
        }
    }
    assert_eq!(slices, 20);
    assert_eq!(apply.done(), work_of(&counts));
}

#[test]
fn cursors_follow_a_batch_cut_only_where_its_parts_cannot_meet_otherwise() {
    let splice = |start, len, text: &str| Splice {
        start,
        len,
        text: (!text.is_empty()).then(|| Rope::from(text)),
    };
    let steps = |batch: Batch| {
        let mut rope = Rope::from("0123456789");
        let mut display = DisplayMap::new(&rope, TabWidth::new(4));
        apply_in_slices(&[batch], &mut rope, &mut display, None, (1, PIECE)).0
    };
    // One splice a slice, but an insertion where a replacement starts stays with it.
    assert_eq!(steps(vec![splice(4, 0, "a"), splice(4, 2, "b")]).len(), 1);
    // Two replacements that meet, and splices with a byte between, may part.
    assert_eq!(steps(vec![splice(2, 2, "a"), splice(4, 2, "b")]).len(), 2);
    assert_eq!(steps(vec![splice(2, 1, "a"), splice(4, 0, "b")]).len(), 2);
}
