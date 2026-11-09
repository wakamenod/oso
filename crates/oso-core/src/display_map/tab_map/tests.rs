//! Tests that a TabMap whose chunks keep checkpoints (ADR 0099) answers as one whose chunks
//! keep none, for every offset and every point of a text.
//!
//! Must not depend on: the editor or a window.

use proptest::prelude::*;

use super::*;
use crate::display_map::checkpoints::Kept;

/// The TabMap of `rope`, its chunks never to make checkpoints.
fn without_checkpoints(rope: &Rope, tab_width: TabWidth) -> TabMap {
    let invalid = InvalidBytes::new();
    let tree = TabMap::items(rope, &invalid, 0..rope.len(), tab_width)
        .map(|mut chunk| {
            chunk.checkpoints = Kept::never();
            chunk
        })
        .collect();
    TabMap {
        tree,
        tab_width,
        invalid,
    }
}

/// Lines of atoms, some past a stride, with tabs, marks, wide characters and line breaks.
fn lines() -> impl Strategy<Value = String> {
    prop::collection::vec(crate::rope::tests::text_strategy(24), 1..6)
        .prop_map(|lines| lines.join("\n"))
}

proptest! {
    #[test]
    fn checkpoints_change_no_answer(text in lines()) {
        let rope = Rope::from(text.as_str());
        let tab_width = TabWidth::new(4);
        let kept = TabMap::new(&rope, tab_width);
        let plain = without_checkpoints(&rope, tab_width);
        for offset in 0..=rope.len() {
            prop_assert_eq!(kept.to_tab_point(offset), plain.to_tab_point(offset), "{}", offset);
            prop_assert_eq!(kept.cluster_at(offset), plain.cluster_at(offset), "{}", offset);
        }
        let end = plain.max_point();
        for row in 0..=end.row {
            let cells = plain.to_tab_point(plain.to_offset(TabPoint::new(row, usize::MAX), Bias::Left)).column;
            for column in 0..=cells + 1 {
                let point = TabPoint::new(row, column);
                for bias in [Bias::Left, Bias::Right] {
                    prop_assert_eq!(kept.locate(point, bias), plain.locate(point, bias), "{:?} {:?}", point, bias);
                }
                let shown = |map: &TabMap| map.clusters(point).map(|c| (c.offset, c.column)).collect::<Vec<_>>();
                prop_assert_eq!(shown(&kept), shown(&plain), "{:?}", point);
            }
        }
    }
}
