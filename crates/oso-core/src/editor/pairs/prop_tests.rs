//! A property of pairs of windows on the editor: whatever is done to the windows, the editor's
//! pairs are the tree's (ADR 0122, step 10).
//!
//! Must not depend on: the host's window.

use super::tests::{ids, lines, on, selected, window};
use super::*;
use crate::buffer::Buffer;

/// What a test does to the windows, at the selected one or the `n`th.
#[derive(Clone, Debug)]
enum Op {
    Split(bool),
    Pair(bool),
    Select(usize),
    Delete,
    DeleteOthers,
    Show(bool),
    Kill(bool),
    Unpair,
    Turn(bool),
    Resize(bool, isize),
    Frame(usize, usize),
    Balance,
    Wheel(usize, isize),
    Sideways(usize, isize),
    Map(usize),
}

/// Maps at the edges: the same lines, gaps on both sides, no runs, runs ending at the last
/// number, and a run that starts past line 0.
fn edge_map(n: usize) -> RowMap {
    let runs = match n % 6 {
        0 => return RowMap::same(),
        1 => vec![(2, 5, 10), (20, 15, 5)],
        2 => Vec::new(),
        3 => vec![(0, usize::MAX - 5, 5)],
        4 => vec![(usize::MAX - 5, 0, 5)],
        _ => vec![(7, 3, 20)],
    };
    RowMap::new(runs).unwrap()
}

fn op() -> impl proptest::strategy::Strategy<Value = Op> {
    use proptest::prelude::*;
    prop_oneof![
        any::<bool>().prop_map(Op::Split),
        any::<bool>().prop_map(Op::Pair),
        (0usize..8).prop_map(Op::Select),
        Just(Op::Delete),
        Just(Op::DeleteOthers),
        any::<bool>().prop_map(Op::Show),
        any::<bool>().prop_map(Op::Kill),
        Just(Op::Unpair),
        any::<bool>().prop_map(Op::Turn),
        (any::<bool>(), -20isize..20).prop_map(|(a, c)| Op::Resize(a, c)),
        (0usize..80, 0usize..260).prop_map(|(r, c)| Op::Frame(r, c)),
        Just(Op::Balance),
        (0usize..8, -40isize..40).prop_map(|(w, r)| Op::Wheel(w, r)),
        (0usize..8, -40isize..40).prop_map(|(w, c)| Op::Sideways(w, c)),
        (0usize..6).prop_map(Op::Map),
    ]
}

/// Does `op` to the windows of `editor`, whose other buffers are `others`.
fn apply(editor: &mut Editor, op: Op, others: [BufferId; 2]) {
    let way = |across: bool| if across { Way::Across } else { Way::Down };
    match op {
        Op::Split(across) => {
            let _ = editor.split_window(way(across), false);
        }
        Op::Pair(across) => {
            let buffer = others[usize::from(across)];
            let _ = editor.pair_window(way(across), buffer, RowMap::same());
        }
        Op::Select(n) => {
            let windows = ids(editor);
            editor.select_window(windows[n % windows.len()]);
        }
        Op::Delete => {
            let _ = editor.delete_window();
        }
        Op::DeleteOthers => {
            editor.delete_other_windows();
        }
        Op::Show(b) => editor.show(others[usize::from(b)]),
        Op::Kill(b) => editor.kill_buffer(others[usize::from(b)]),
        Op::Unpair => {
            editor.unpair(selected(editor));
        }
        Op::Turn(across) => {
            let _ = editor.turn_pair(selected(editor), way(across));
        }
        Op::Resize(across, cells) => {
            let _ = editor.resize_window(way(across), cells);
        }
        Op::Frame(rows, columns) => {
            editor.set_frame_size(crate::frame::FrameSize { rows, columns });
        }
        Op::Balance => {
            editor.balance_windows();
        }
        Op::Wheel(n, rows) => {
            let windows = ids(editor);
            editor.scroll_view(windows[n % windows.len()], rows);
        }
        Op::Sideways(n, columns) => {
            let windows = ids(editor);
            editor.scroll_view_sideways(windows[n % windows.len()], columns);
        }
        Op::Map(n) => {
            editor.set_row_map(selected(editor), edge_map(n));
        }
    }
}

proptest::proptest! {
    /// Whatever is done to the windows, the editor's pairs are the tree's, each window of one
    /// shows what it was paired with, and the selected window is a tiled one.
    #[test]
    fn prop_the_pairs_are_the_trees(ops in proptest::collection::vec(op(), 0..30)) {
        let mut editor = on(&lines(100), (60, 241));
        let others = [
            editor.add_buffer(Buffer::from_text("a", &lines(50))),
            editor.add_buffer(Buffer::from_text("b", &lines(70))),
        ];
        for op in ops {
            apply(&mut editor, op, others);
            editor.follow_cursor();
            let mut records: Vec<[WindowId; 2]> = editor.pairs.iter().map(|p| p.windows).collect();
            let mut tree = editor.splits.pairs();
            records.sort();
            tree.sort();
            proptest::prop_assert_eq!(records, tree);
            for pair in &editor.pairs {
                for (id, buffer) in pair.windows.iter().zip(pair.buffers) {
                    proptest::prop_assert_eq!(window(&editor, *id).buffer(), buffer);
                }
            }
            proptest::prop_assert!(ids(&editor).contains(&selected(&editor)));
        }
    }
}
