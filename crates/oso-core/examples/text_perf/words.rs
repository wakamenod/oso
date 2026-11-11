//! Word motion at many cursors, which share one command's reading, and TAB at many cursors
//! on one long row, whose columns come from the display map (ADR 0101). Run with `-- words`.
//!
//! Must not depend on: crate internals; it uses only the public API, and the text and
//! timing helpers of `text_perf`.

use std::time::{Duration, Instant};

use oso_core::display_map::{DisplayMap, DisplaySnapshot, TabWidth};
use oso_core::editor::Editor;
use oso_core::motion::{self, WalkBudget};
use oso_core::rope::Rope;
use oso_core::word;

use super::{MB, editor_with_cursors, file_text, floor_boundary, percentiles, single_line};

/// `cursors` cursor starts spread over `text`, on cluster starts.
fn starts(display: &DisplaySnapshot, text: &str, cursors: usize) -> Vec<usize> {
    let stride = text.len() / cursors;
    (0..cursors)
        .map(|index| motion::cluster_start(display, floor_boundary(text, index * stride)))
        .collect()
}

/// p50 and p99 of one word key at each of `cursors` cursors spread over `text`, each way,
/// with the budget a command at that many has, and how many cursors the last key moved.
fn cursor_word_costs(label: &str, text: &str) {
    let display = DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot();
    for cursors in [1, 4, 16, 64, 256, 1000] {
        let starts = starts(&display, text, cursors);
        let key = |n: isize| {
            let mut budget = WalkBudget::for_cursors(cursors);
            let start = Instant::now();
            let moved = starts
                .iter()
                .filter(|&&at| word::move_words(&display, at, n, &mut budget) != at)
                .count();
            (start.elapsed(), moved)
        };
        let times = |n: isize| {
            let mut moved = 0;
            let mut samples: Vec<Duration> = (0..20)
                .map(|_| {
                    let (elapsed, count) = key(n);
                    moved = count;
                    elapsed
                })
                .collect();
            let (p50, p99) = percentiles(&mut samples);
            (p50, p99, moved)
        };
        let (forward_50, forward_99, forward_moved) = times(1);
        let (backward_50, backward_99, backward_moved) = times(-1);
        println!(
            "{label:<20} {cursors:>3} cursors  M-f p50 {forward_50:>9.2?} p99 {forward_99:>9.2?} ({forward_moved} moved)   M-b p50 {backward_50:>9.2?} p99 {backward_99:>9.2?} ({backward_moved} moved)"
        );
    }
}

/// TAB at every cursor of `editor`, as `tab-to-tab-stop` runs it, in one undo step.
fn tab_at_cursors(editor: &mut Editor) {
    editor.begin_undo_group();
    editor.tab_to_tab_stop().expect("the buffer is editable");
    editor.end_undo_group();
}

/// p50 and p99 of TAB at `cursors` cursors on one row of `text`.
fn tab_costs(label: &str, text: &str) {
    for cursors in [2, 16, 100, 1000] {
        let mut editor = editor_with_cursors(text, cursors);
        let mut samples: Vec<Duration> = (0..50)
            .map(|_| {
                let start = Instant::now();
                tab_at_cursors(&mut editor);
                start.elapsed()
            })
            .collect();
        let (p50, p99) = percentiles(&mut samples);
        println!("{label:<20} {cursors:>3} cursors  TAB p50 {p50:>9.2?} p99 {p99:>9.2?}");
    }
}

/// Word motion at many cursors, and TAB at many cursors on one row.
pub(super) fn cursor_costs() {
    println!("== Word motion at several cursors, one key, sharing one command's reading");
    cursor_word_costs("10 MB file", &file_text(10 * MB));
    cursor_word_costs("10 MB line", &single_line(10 * MB));
    cursor_word_costs("10 MB line of x", &"x".repeat(10 * MB));
    cursor_word_costs("10 MB line of д", &"д".repeat(5 * MB));
    println!();
    println!("== TAB at several cursors on one row");
    tab_costs("10 MB line", &single_line(10 * MB));
    tab_costs("10 MB line of x", &"x".repeat(10 * MB));
}
