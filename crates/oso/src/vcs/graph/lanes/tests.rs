//! Tests of a graph's lanes: a line of history, a fork and its merge drawn as git draws them,
//! and property tests over any graph in topological order: every parent edge is kept by some
//! lane until its parent's row, each commit's marker sits in its own lane, every open lane
//! expects a commit still to come, and nothing panics on ids that repeat or parents never listed.
//!
//! Must not depend on: git or the file system.

use proptest::prelude::*;

use super::{Lanes, MARKER, MAX_LANES, MORE};

fn text(cells: &[super::Cell]) -> String {
    cells
        .iter()
        .map(|cell| cell.ch)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

fn ids(parents: &[&str]) -> Vec<String> {
    parents.iter().map(|id| (*id).to_owned()).collect()
}

/// The rows of `graph`, each `(id, parents)`, newest first.
fn rows(graph: &[(&str, &[&str])]) -> Vec<String> {
    let mut lanes = Lanes::default();
    graph
        .iter()
        .map(|(id, parents)| text(&lanes.row(id, &ids(parents)).0))
        .collect()
}

#[test]
fn a_line_of_history_is_one_lane() {
    assert_eq!(
        rows(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]),
        ["●", "●", "●"]
    );
}

#[test]
fn a_merge_forks_and_its_parents_join_again() {
    // m merges b into a's line; both come from base.
    let drawn = rows(&[
        ("m", &["a", "b"]),
        ("b", &["base"]),
        ("a", &["base"]),
        ("base", &[]),
    ]);
    assert_eq!(drawn, ["●─╮", "│ ●", "● │", "●─╯"]);
}

#[test]
fn two_branch_tips_take_two_lanes() {
    let drawn = rows(&[("x", &["base"]), ("y", &["base"]), ("base", &[])]);
    assert_eq!(drawn, ["●", "│ ●", "●─╯"]);
}

#[test]
fn past_the_most_lanes_one_cell_says_there_are_more() {
    let mut lanes = Lanes::default();
    let tips: Vec<String> = (0..MAX_LANES + 3).map(|n| format!("t{n}")).collect();
    let mut last = Vec::new();
    for tip in &tips {
        last = lanes.row(tip, &ids(&["base"])).0;
    }
    let shown = text(&last);
    assert!(
        shown.chars().filter(|c| *c == '│').count() <= MAX_LANES,
        "{shown}"
    );
    assert!(shown.contains(MORE) || shown.contains(MARKER), "{shown}");
    let (base, _) = lanes.row("base", &[]);
    assert!(base.len() <= 2 * MAX_LANES + 2);
}

/// Any graph of up to 40 commits in topological order: each commit's parents come after it.
fn graphs() -> impl Strategy<Value = Vec<Vec<usize>>> {
    (1usize..40).prop_flat_map(|n| {
        let rows: Vec<_> = (0..n)
            .map(|at| {
                let later = (at + 1)..n.max(at + 2);
                prop::collection::vec(later, 0..4)
            })
            .collect();
        rows
    })
}

proptest! {
    #[test]
    fn every_parent_edge_is_kept_until_its_parent_s_row(graph in graphs()) {
        let n = graph.len();
        let id = |at: usize| format!("c{at}");
        let mut lanes = Lanes::default();
        let mut children = vec![0usize; n + 1];
        for (at, parents) in graph.iter().enumerate() {
            let parents: Vec<usize> = parents.iter().copied().filter(|p| *p < n).collect();
            // A commit with a child already drawn is expected by a lane.
            if children[at] > 0 {
                prop_assert!(lanes.expects(&id(at)), "c{at} lost its children's edge");
            }
            let names: Vec<String> = parents.iter().map(|p| id(*p)).collect();
            let (cells, lane) = lanes.row(&id(at), &names);
            if lane < MAX_LANES {
                prop_assert_eq!(cells[2 * lane].ch, MARKER);
            }
            prop_assert_eq!(cells.iter().filter(|cell| cell.ch == MARKER).count(), 1);
            for parent in &parents {
                children[*parent] += 1;
                prop_assert!(lanes.expects(&id(*parent)));
            }
            // No lane is open but for a commit still to come.
            for expected in lanes.expected() {
                let later: usize = expected[1..].parse().unwrap();
                prop_assert!(later > at && later < n, "a lane expects {expected} after c{at}");
            }
        }
    }

    #[test]
    fn any_ids_never_panic(rows in prop::collection::vec(
        ("[a-c]{1,2}", prop::collection::vec("[a-c]{1,2}", 0..4)), 0..30)
    ) {
        let mut lanes = Lanes::default();
        for (id, parents) in &rows {
            let (cells, _) = lanes.row(id, parents);
            prop_assert!(!cells.is_empty());
        }
    }
}
