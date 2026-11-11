//! Word motion one key at a time and with a huge count, word motion at many cursors, which
//! share one command's reading, and TAB at many cursors on one long row, whose columns come
//! from the display map (ADR 0101). Run with `-- words`.
//!
//! Must not depend on: crate internals; it uses only the public API, and the text and
//! timing helpers of `text_perf`.

use std::hint::black_box;
use std::time::{Duration, Instant};

use oso_core::display_map::{DisplayMap, DisplaySnapshot, TabWidth};
use oso_core::editor::Editor;
use oso_core::motion::{self, WalkBudget};
use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;
use oso_core::word;

use super::{
    Lcg, MB, editor_with_cursors, file_text, floor_boundary, percentiles, single_line, time,
};

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

/// The longest and the middle of some keys' worth of word motion from `start`, each
/// way, with a new budget for each key, as commands have.
fn word_costs(label: &str, text: &str) {
    let display = DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot();
    let times = |forward: bool| {
        let mut rng = Lcg(5);
        let mut samples: Vec<Duration> = (0..200)
            .map(|_| {
                let at = floor_boundary(text, rng.below(text.len()));
                let at = display
                    .display_point_to_offset(display.offset_to_display_point(at), Bias::Left);
                let mut budget = WalkBudget::new();
                time(|| {
                    black_box(if forward {
                        word::forward_word(&display, at, &mut budget)
                    } else {
                        word::backward_word(&display, at, &mut budget)
                    })
                })
                .1
            })
            .collect();
        percentiles(&mut samples)
    };
    let (forward_p50, forward_p99) = times(true);
    let (backward_p50, backward_p99) = times(false);
    println!(
        "{label:<28} M-f p50 {forward_p50:>9.2?} p99 {forward_p99:>9.2?}   M-b p50 {backward_p50:>9.2?} p99 {backward_p99:>9.2?}"
    );
}

/// A count that reads all a command may, each way.
fn word_count_costs(label: &str, text: &str) {
    let display = DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot();
    let (forward, forward_time) = time(|| {
        let mut budget = WalkBudget::new();
        word::move_words(&display, 0, isize::MAX, &mut budget)
    });
    let (backward, backward_time) = time(|| {
        let mut budget = WalkBudget::new();
        word::move_words(&display, text.len(), isize::MIN, &mut budget)
    });
    println!(
        "{label:<28} huge count: forward {forward_time:>9.2?} to {forward}, backward {backward_time:>9.2?} to {backward}"
    );
}

/// What word motion costs, on ordinary text and on runs with no boundary, and at several
/// cursors with TAB beside it. Run with the argument `words` for this alone.
pub(super) fn word_motion_costs() {
    println!("== Word motion, one key at a time");
    word_costs("100 MB file", &file_text(100 * MB));
    word_costs("10 MB line", &single_line(10 * MB));
    word_costs("10 MB line of x", &"x".repeat(10 * MB));
    word_costs("10 MB line of 語", &"語".repeat(10 * MB / 3));
    word_costs("10 MB line of spaces", &" ".repeat(10 * MB));
    word_costs("10 MB of empty lines", &"\n".repeat(10 * MB));
    word_costs(
        "10 MB of marks",
        &("e".to_owned() + &"\u{301}".repeat(5 * MB)),
    );
    word_costs(
        "10 MB of e + 8 marks",
        &format!("e{}", "\u{301}".repeat(8)).repeat(MB / 2),
    );
    let chain = ["👨"; 8].join("\u{200d}");
    word_costs("10 MB of ZWJ chains", &chain.repeat(10 * MB / chain.len()));
    word_costs("10 MB line of д", &"д".repeat(5 * MB));
    word_costs("10 MB line of 한", &"한".repeat(10 * MB / 3));
    word_costs("10 MB line of ZWSP", &"\u{200b}".repeat(10 * MB / 3));
    word_costs(
        "10 MB of rows of 、",
        &("、".repeat(300) + "\n").repeat(10 * MB / 901),
    );
    println!();
    println!("== Word motion with a count past what a command reads");
    word_count_costs("100 MB file", &file_text(100 * MB));
    word_count_costs("10 MB of empty lines", &"\n".repeat(10 * MB));
    word_count_costs("10 MB of `a `", &"a ".repeat(5 * MB));
    word_count_costs("10 MB line of 語", &"語".repeat(10 * MB / 3));
    let chain = ["👨"; 8].join("\u{200d}");
    word_count_costs("10 MB of ZWJ chains", &chain.repeat(10 * MB / chain.len()));
    println!();
    cursor_costs();
}
