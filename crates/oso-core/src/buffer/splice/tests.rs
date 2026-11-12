//! Tests of applying batches (ADR 0053): what a batch rebuilt and changed, and an undo group's
//! batches that lie apart applied as one (ADR 0119).
//!
//! Must not depend on: `window`, `editor` or `command`; these see only text.

use std::collections::HashSet;

use proptest::prelude::*;

use super::super::edited::Edited;
use super::*;
use crate::display_map::TabWidth;
use crate::rope::tests::{ATOMS, floor_boundary, text_strategy};

/// Each chunk of `rope`, with where it starts.
fn chunks(rope: &Rope) -> Vec<(usize, String)> {
    let mut at = 0;
    rope.chunk_texts(0..rope.len())
        .map(|text| {
            let start = at;
            at += text.len();
            (start, text.to_string())
        })
        .collect()
}

/// Splices from `raw`, at character boundaries of `text`, sorted and apart.
fn batch<'a>(text: &str, raw: Vec<(usize, usize, &'a str)>) -> Vec<Splice<Cow<'a, str>>> {
    batch_of(text, raw, false)
}

/// As [`batch`], and with `touching`, splices may also meet, as [`normalized`] lets them.
fn batch_of<'a>(
    text: &str,
    raw: Vec<(usize, usize, &'a str)>,
    touching: bool,
) -> Vec<Splice<Cow<'a, str>>> {
    let mut cut: Vec<(usize, usize, &str)> = raw
        .into_iter()
        .map(|(at, len, new)| {
            let start = floor_boundary(text, at);
            (start, floor_boundary(text, start + len), new)
        })
        .collect();
    cut.sort_by_key(|&(start, end, _)| (start, end));
    let mut splices: Vec<Splice<Cow<'a, str>>> = Vec::new();
    for (start, end, new) in cut {
        let apart = splices.last().is_none_or(|last| {
            let end_of_last = last.start + last.len;
            // Two insertions at one offset overlap; a splice that starts where one ends meets it.
            start > end_of_last
                || (touching && start == end_of_last && (last.len > 0 || end > start))
        });
        if apart && (start < end || !new.is_empty()) {
            splices.push(Splice {
                start,
                len: end - start,
                text: Cow::Borrowed(new),
            });
        }
    }
    splices
}

/// Where `offset`, outside every splice, is once they have applied; `None` inside one,
/// or at its end if `inclusive`.
fn moved<T: SpliceText>(splices: &[Splice<T>], offset: usize, inclusive: bool) -> Option<usize> {
    let mut delta = 0isize;
    for splice in splices {
        let (start, end) = (splice.start, splice.start + splice.len);
        if offset < start {
            break;
        }
        let after = if inclusive {
            offset > end
        } else {
            offset >= end
        };
        if !after {
            return None;
        }
        delta += splice.text.byte_len() as isize - splice.len as isize;
    }
    offset.checked_add_signed(delta)
}

/// Checks that every chunk the batch rebuilt is in `changed`.
fn rebuilt_chunks_are_changed(text: &str, splices: &[Splice<Cow<'_, str>>]) {
    let mut rope = Rope::from(text);
    let kept: HashSet<(usize, String)> = chunks(&rope)
        .into_iter()
        .filter_map(|(start, chunk)| {
            let end = start + chunk.len();
            let outside = splices
                .iter()
                .all(|splice| end <= splice.start || start >= splice.start + splice.len);
            outside
                .then(|| moved(splices, start, false))
                .flatten()
                .map(|start| (start, chunk))
        })
        .collect();
    let mut display = DisplayMap::new(&rope, TabWidth::new(4));
    let mut changed = Vec::new();
    apply_marking(splices, &mut rope, &mut display, &mut changed, None);
    for (start, chunk) in chunks(&rope) {
        if kept.contains(&(start, chunk.clone())) {
            continue;
        }
        let end = start + chunk.len();
        assert!(
            changed
                .iter()
                .any(|range| range.start <= start && end <= range.end),
            "chunk {start}..{end} is not in {changed:?}"
        );
    }
}

#[test]
fn chunks_rebuilt_back_past_the_splice_below_are_changed() {
    let text = "aのaのaa👍\r\na👍\r\n\r\n\u{7f}\r\n\u{e0041}🇯🇵\r\n日本語aaa\r\na\u{200b}\r\n\r\nの\r\nの\r\na日本語\u{e0041}の👨\u{200d}👩\u{200d}👧日本語a";
    rebuilt_chunks_are_changed(text, &batch(text, vec![(95, 0, "a"), (109, 0, "a")]));
}

/// A batch that deletes `len` bytes at each of `starts` and puts `new` there.
fn replacing(starts: &[usize], len: usize, new: &str) -> Batch {
    starts
        .iter()
        .map(|&start| Splice {
            start,
            len,
            text: (!new.is_empty()).then(|| Rope::from(new)),
        })
        .collect()
}

#[test]
fn batches_apart_join_and_those_that_meet_do_not() {
    // Undoing answers made forward applies each before the last; the group holds them
    // first to last, and applies them last first.
    let back: Vec<Batch> = (0..100)
        .map(|line| replacing(&[line * 10], 6, "abcdef"))
        .collect();
    assert_eq!(joined_apart(&back).len(), 1);
    // Redoing them applies each after what the last became, put in the offsets before.
    let on: Vec<Batch> = (0..100)
        .rev()
        .map(|line| replacing(&[line * 10], 6, "ab"))
        .collect();
    let runs = joined_apart(&on);
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0][1].start, runs[0][1].len), (14, 6));
    // A batch that touches what was joined, or overlaps it, starts a run of its own.
    let touching = vec![
        replacing(&[8], 0, "a"),
        replacing(&[8], 2, ""),
        replacing(&[10], 2, ""),
    ];
    assert_eq!(joined_apart(&touching).len(), 3);
    // An insertion right where the text the batch before put in ends meets it, so it is
    // not joined, and the group still gives the text one after another would.
    let meeting = vec![replacing(&[4], 0, "b"), replacing(&[2], 2, "XY")];
    assert_eq!(joined_apart(&meeting).len(), 2);
    let mut rope = Rope::from("0123456789");
    let mut display = DisplayMap::new(&rope, TabWidth::new(4));
    apply_group(&meeting, &mut rope, &mut display, None);
    assert_eq!(rope.to_string(), "01XYb456789");
    let one = vec![replacing(&[1, 5], 1, "x")];
    assert!(
        matches!(joined_apart(&one)[0], Cow::Borrowed(_)),
        "one batch is not copied"
    );
    assert!(joined_apart(&[Vec::new(), Vec::new()]).is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// A group applies as its batches would one after another, with batches apart joined:
    /// the same text, offsets moved the same way, and what it gives back undoes it
    /// (ADR 0119).
    #[test]
    fn a_group_applies_as_its_batches_one_after_another(
        text in text_strategy(60),
        raws in prop::collection::vec(
            prop::collection::vec((0usize..120, 0usize..6, prop::sample::select(ATOMS)), 1..4),
            1..10,
        ),
        marks in prop::collection::vec(0usize..140, 0..8),
    ) {
        // The batches in the order they apply, each on the text the ones before left.
        let mut now = text.clone();
        let mut applied: Vec<Batch> = Vec::new();
        for raw in raws {
            let splices = batch_of(&now, raw, true);
            let mut rope = Rope::from(now.as_str());
            let mut display = DisplayMap::new(&rope, TabWidth::new(4));
            apply(&splices, &mut rope, &mut display);
            now = rope.to_string();
            applied.push(
                splices
                    .iter()
                    .map(|splice| Splice {
                        start: splice.start,
                        len: splice.len,
                        text: (!splice.text.is_empty()).then(|| Rope::from(&*splice.text)),
                    })
                    .collect(),
            );
        }
        let group: Vec<Batch> = applied.iter().rev().cloned().collect();
        let mut rope = Rope::from(text.as_str());
        let mut display = DisplayMap::new(&rope, TabWidth::new(4));
        // The cluster starts no batch touched, where they are once all have applied.
        let before = display.snapshot();
        let starts: Vec<usize> = (0..=text.len())
            .filter(|&at| text.is_char_boundary(at) && before.cluster_at(at).0 == at)
            .filter_map(|at| {
                applied.iter().try_fold(at, |at, batch| moved(batch, at, true))
            })
            .collect();
        let (steps, back, changed) = apply_group(&group, &mut rope, &mut display, None);
        prop_assert_eq!(rope.to_string(), now.clone());
        prop_assert!(steps.len() <= applied.len());
        // What the joined batches say changed holds every start that may have moved.
        let after = display.snapshot();
        for at in starts {
            let index = changed.partition_point(|range| range.end < at);
            if changed.get(index).is_some_and(|range| range.start <= at) {
                continue;
            }
            prop_assert_eq!(after.cluster_at(at).0, at, "changed {:?}", changed);
        }
        let mut marks: Vec<Range<usize>> = marks.into_iter().map(|at| at..at + 3).collect();
        marks.sort_unstable_by_key(|range| range.start);
        let mut joined = marks.clone();
        // The lines whitespace cleanup looks at follow the same way (ADR 0113).
        let mut edited = Edited::default();
        edited.follow(&marks.iter().map(|range| OffsetEdit {
            start: range.start,
            old_len: 0,
            new_len: 0,
        }).collect::<Vec<_>>());
        let mut edited_joined = edited.clone();
        for batch in &applied {
            let edits: Vec<_> = batch.iter().map(offset_edit).collect();
            follow_ranges(&mut marks, &edits);
            edited.follow(&edits);
        }
        for step in &steps {
            follow_ranges(&mut joined, step);
            edited_joined.follow(step);
        }
        prop_assert_eq!(joined, marks);
        prop_assert_eq!(edited_joined.ranges(), edited.ranges());
        // Undone and redone, as an undo and a redo cross a group and the one it gives back.
        let (_, again, _) = apply_group(&back, &mut rope, &mut display, None);
        prop_assert_eq!(rope.to_string(), text);
        apply_group(&again, &mut rope, &mut display, None);
        prop_assert_eq!(rope.to_string(), now);
    }

    /// Batches that each lie wholly before the last, as undoing answers made forward applies
    /// them, join into one step, and so do those that each lie wholly after what the last
    /// became, as redoing them applies them (ADR 0119).
    #[test]
    fn batches_each_apart_from_the_last_join_into_one(
        gaps in prop::collection::vec((1usize..5, 0usize..3, 0usize..3), 2..12),
    ) {
        let text = "x".repeat(200);
        let mut at = 0;
        let forward: Vec<Batch> = gaps
            .iter()
            .map(|&(gap, len, new)| {
                let splice = Splice { start: at + gap, len, text: (new > 0).then(|| Rope::from("y".repeat(new).as_str())) };
                at = splice.start + new;
                vec![splice]
            })
            .collect();
        // Made forward, each a byte or more after what the last became.
        let group: Vec<Batch> = forward.iter().rev().cloned().collect();
        let mut rope = Rope::from(text.as_str());
        let mut display = DisplayMap::new(&rope, TabWidth::new(4));
        let (steps, back, _) = apply_group(&group, &mut rope, &mut display, None);
        prop_assert_eq!(steps.len(), 1);
        // Undoing them applies each before the last.
        let (steps, _, _) = apply_group(&back, &mut rope, &mut display, None);
        prop_assert_eq!(steps.len(), 1);
        prop_assert_eq!(rope.to_string(), text);
    }

    /// Every chunk a batch rebuilt is in what it says changed (ADR 0091).
    #[test]
    fn every_rebuilt_chunk_is_changed(
        text in text_strategy(60),
        raw in prop::collection::vec((0usize..400, 0usize..6, prop::sample::select(ATOMS)), 1..6),
    ) {
        let splices = batch(&text, raw);
        prop_assume!(!splices.is_empty());
        rebuilt_chunks_are_changed(&text, &splices);
    }

    /// A cluster start outside every splice and outside what changed is still one,
    /// also among long runs of marks and of regional indicators.
    #[test]
    fn what_did_not_change_still_starts_its_clusters(
        text in prop::collection::vec(
            prop_oneof![
                3 => prop::sample::select(ATOMS).prop_map(str::to_owned),
                3 => (10usize..60).prop_map(|n| format!("a{}", "\u{301}".repeat(n))),
                1 => (40usize..160).prop_map(|n| "\u{1f1ef}".repeat(n)),
            ],
            0..30,
        ).prop_map(|atoms| atoms.concat()),
        raw in prop::collection::vec((0usize..1200, 0usize..6, prop::sample::select(ATOMS)), 1..8),
    ) {
        let splices = batch(&text, raw);
        let mut rope = Rope::from(text.as_str());
        let mut display = DisplayMap::new(&rope, TabWidth::new(4));
        let before = display.snapshot();
        let starts: Vec<usize> = (0..=text.len())
            .filter(|&at| text.is_char_boundary(at) && before.cluster_at(at).0 == at)
            .filter_map(|at| moved(&splices, at, true))
            .collect();
        let mut changed = Vec::new();
        apply_marking(&splices, &mut rope, &mut display, &mut changed, None);
        let after = display.snapshot();
        for at in starts {
            let index = changed.partition_point(|range| range.end < at);
            if changed.get(index).is_some_and(|range| range.start <= at) {
                continue;
            }
            prop_assert_eq!(after.cluster_at(at).0, at, "changed {:?}", changed);
        }
    }
}
