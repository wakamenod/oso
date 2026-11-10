//! Tests that a TabMap whose chunks keep checkpoints (ADR 0099) answers as a scan of every
//! chunk from its start does: points, offsets, clusters and rows, with invalid bytes, any
//! tab width, after edits, in snapshots taken before them, and read from several threads;
//! and the columns after an insertion that `column_after` tells (ADR 0101).
//!
//! Must not depend on: the editor or a window.

use proptest::prelude::*;

use super::*;
use crate::display_map::checkpoints::tests::{invalid_in, pieces};

/// A cluster as a scan of its chunk from the chunk's start finds it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Scanned {
    start: usize,
    end: usize,
    point: TabPoint,
    cells: usize,
    kind: CellKind,
}

/// Every cluster of `rope`, each chunk scanned from its start, and the point at its end.
fn scanned(rope: &Rope, invalid: &InvalidBytes, tab_width: TabWidth) -> (Vec<Scanned>, TabPoint) {
    let mut out = Vec::new();
    let mut point = TabPoint::default();
    let mut base = 0;
    for text in rope.chunk_texts(0..rope.len()) {
        for (index, cluster, kind, cells) in Clusters::with_invalid(text, base, invalid) {
            let cells = match kind {
                CellKind::Tab => tab_width.next_stop(point.column) - point.column,
                CellKind::LineBreak => 0,
                _ => cells,
            };
            out.push(Scanned {
                start: base + index,
                end: base + index + cluster.len(),
                point,
                cells,
                kind,
            });
            match kind {
                CellKind::LineBreak => point = TabPoint::new(point.row + 1, 0),
                _ => point.column += cells,
            }
        }
        base += text.len();
    }
    (out, point)
}

/// Whether `map` answers for `rope` as the scans from chunk starts do, at every offset and
/// at every column of every row, with either bias.
fn answers_as_scanned(
    map: &TabMap,
    rope: &Rope,
    invalid: &InvalidBytes,
    tab_width: TabWidth,
) -> Result<(), TestCaseError> {
    let (clusters, end) = scanned(rope, invalid, tab_width);
    let len = rope.len();
    // Twice, so that the second pass reads chunks that have made their checkpoints.
    for _ in 0..2 {
        for offset in 0..=len {
            let around = clusters
                .iter()
                .find(|cluster| cluster.start <= offset && offset < cluster.end);
            let point = around.map_or(end, |cluster| cluster.point);
            prop_assert_eq!(map.to_tab_point(offset), point, "point of {}", offset);
            let expected = match around {
                Some(cluster) if cluster.start != offset => (cluster.start, cluster.end),
                _ => (offset, offset),
            };
            prop_assert_eq!(map.cluster_at(offset), expected, "cluster at {}", offset);
        }
        let rows = end.row + 1;
        for row in 0..=rows {
            rows_answer(map, &clusters, (row, rows), len)?;
        }
    }
    Ok(())
}

/// [`answers_as_scanned`] for each column of `row`, one of `rows`.
fn rows_answer(
    map: &TabMap,
    clusters: &[Scanned],
    (row, rows): (usize, usize),
    len: usize,
) -> Result<(), TestCaseError> {
    let line: Vec<&Scanned> = clusters
        .iter()
        .filter(|cluster| cluster.point.row == row && cluster.kind != CellKind::LineBreak)
        .collect();
    let width = line
        .last()
        .map_or(0, |cluster| cluster.point.column + cluster.cells);
    let line_end = clusters
        .iter()
        .find(|cluster| cluster.point.row == row && cluster.kind == CellKind::LineBreak)
        .map_or(len, |cluster| cluster.start);
    for column in 0..=width + 2 {
        let target = TabPoint::new(row, column);
        for bias in [Bias::Left, Bias::Right] {
            let expected = if row >= rows {
                len
            } else {
                line.iter()
                    .find(|cluster| column < cluster.point.column + cluster.cells)
                    .map_or(line_end, |cluster| {
                        if column == cluster.point.column || bias == Bias::Left {
                            cluster.start
                        } else {
                            cluster.end
                        }
                    })
            };
            prop_assert_eq!(
                map.to_offset(target, bias),
                expected,
                "{:?} {:?}",
                target,
                bias
            );
        }
        if row < rows {
            let shown: Vec<_> = map
                .clusters(target)
                .map(|c| (c.offset, c.offset + c.text.len(), c.column, c.cells, c.kind))
                .collect();
            let expected: Vec<_> = line
                .iter()
                .filter(|cluster| column < cluster.point.column + cluster.cells)
                .map(|c| (c.start, c.end, c.point.column, c.cells, c.kind))
                .collect();
            prop_assert_eq!(shown, expected, "clusters from {:?}", target);
        }
    }
    Ok(())
}

proptest! {
    #[test]
    fn checkpoints_answer_as_scans_from_chunk_starts(
        text in pieces(),
        tab_width in 1u32..10,
        every in 1usize..3,
    ) {
        let tab_width = TabWidth::new(tab_width);
        let rope = Rope::from(text.as_str());
        let invalid = invalid_in(&text, every);
        let map = TabMap::with_invalid(&rope, &invalid, tab_width);
        answers_as_scanned(&map, &rope, &invalid, tab_width)?;
    }
}

proptest! {
    // Each case checks every answer after each edit, and in the snapshot before it.
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn checkpoints_follow_edits_and_stay_in_earlier_snapshots(
        text in pieces(),
        edits in prop::collection::vec((0usize..400, 0usize..400, pieces()), 1..4),
        tab_width in 1u32..10,
    ) {
        let tab_width = TabWidth::new(tab_width);
        let invalid = InvalidBytes::new();
        let mut rope = Rope::from(text.as_str());
        let mut map = TabMap::new(&rope, tab_width);
        answers_as_scanned(&map, &rope, &invalid, tab_width)?;
        for (a, b, insert) in edits {
            let model = rope.to_string();
            let start = crate::rope::tests::floor_boundary(&model, a.min(b));
            let end = crate::rope::tests::floor_boundary(&model, a.max(b));
            let (before, before_rope) = (map.clone(), rope.clone());
            let edit = rope.replace(start..end, &insert);
            map.sync(&rope, &edit);
            answers_as_scanned(&map, &rope, &invalid, tab_width)?;
            answers_as_scanned(&before, &before_rope, &invalid, tab_width)?;
        }
    }
}

/// Whether [`TabMap::column_after`] answers as a walk of the clusters between does, for
/// every pair of cluster starts on one line, `to` also at the line's end, with `from` at
/// each column in `columns`.
fn columns_after_answer_as_walked(
    map: &TabMap,
    clusters: &[Scanned],
    tab_width: TabWidth,
    columns: &[usize],
) -> Result<(), TestCaseError> {
    for (index, from) in clusters.iter().enumerate() {
        if from.kind == CellKind::LineBreak {
            continue;
        }
        for &column in columns {
            let mut walked = column;
            for to in &clusters[index..] {
                prop_assert_eq!(
                    map.column_after(from.start, column, to.start),
                    walked,
                    "from {} at {} to {}",
                    from.start,
                    column,
                    to.start
                );
                if to.kind == CellKind::LineBreak {
                    break;
                }
                walked = match to.kind {
                    CellKind::Tab => tab_width.next_stop(walked),
                    _ => walked + to.cells,
                };
            }
            if clusters[index..]
                .iter()
                .all(|to| to.kind != CellKind::LineBreak)
            {
                let end = clusters.last().map_or(0, |last| last.end);
                prop_assert_eq!(map.column_after(from.start, column, end), walked);
            }
        }
    }
    Ok(())
}

proptest! {
    // Each case checks every pair of cluster starts on a line.
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn column_after_answers_as_a_walk_between(
        text in pieces(),
        tab_width in 1u32..10,
        columns in prop::collection::vec(0usize..40, 1..4),
    ) {
        let tab_width = TabWidth::new(tab_width);
        let rope = Rope::from(text.as_str());
        let invalid = invalid_in(&text, 1);
        let map = TabMap::with_invalid(&rope, &invalid, tab_width);
        let (clusters, _) = scanned(&rope, &invalid, tab_width);
        // Twice, so that the second pass reads chunks that have made their checkpoints.
        for _ in 0..2 {
            columns_after_answer_as_walked(&map, &clusters, tab_width, &columns)?;
        }
    }
}

#[test]
fn snapshots_read_from_several_threads_share_one_build() {
    let text = "日本\tx\u{301}".repeat(200);
    let rope = Rope::from(text.as_str());
    let map = TabMap::new(&rope, TabWidth::new(4));
    let (points, around): (Vec<_>, Vec<_>) = (0..text.len())
        .map(|offset| (map.to_tab_point(offset), map.cluster_at(offset)))
        .unzip();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let (map, points, around, len) = (map.clone(), &points, &around, text.len());
            scope.spawn(move || {
                for offset in (0..len).step_by(7) {
                    assert_eq!(map.to_tab_point(offset), points[offset]);
                    assert_eq!(map.cluster_at(offset), around[offset]);
                }
            });
        }
    });
}
