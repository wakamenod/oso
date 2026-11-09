//! The tests of [`WrapMap`](super::WrapMap): what a budget leaves stale is rewrapped by the
//! jobs, a newer width supersedes a job, and a report takes the commands since.
//!
//! Must not depend on: threads; the jobs run where the test takes them.

use proptest::prelude::*;

use super::*;
use crate::display_map::{DisplayMap, TabWidth};
use crate::rope::Rope;

/// A buffer, its display map and a window's wrap of it.
struct Window {
    rope: Rope,
    map: DisplayMap,
    wrap: WrapMap,
}

impl Window {
    fn new(text: &str, columns: usize) -> Self {
        let rope = Rope::from(text);
        let map = DisplayMap::new(&rope, TabWidth::new(4));
        let wrap = WrapMap::new(&map.snapshot(), columns);
        Self { rope, map, wrap }
    }

    /// One command of replacements, given in descending order as the editor applies them.
    fn command(&mut self, replacements: &[(usize, usize, &str)]) {
        let mut splices = Vec::new();
        for &(start, len, text) in replacements {
            let now = self.rope.to_string();
            let start = crate::rope::tests::floor_boundary(&now, start);
            let end = crate::rope::tests::floor_boundary(&now, start + len);
            let edit = self.rope.replace(start..end, text);
            let before = self.map.snapshot();
            self.map.sync(&self.rope, &edit);
            splices.push(Splice {
                before,
                after: self.map.snapshot(),
                edit,
            });
        }
        self.wrap.edit(&splices);
    }

    /// Runs jobs until nothing is stale.
    fn settle(&mut self) {
        while let Some(job) = self.wrap.take_job(&self.map.snapshot()) {
            if let Some(report) = job.run() {
                self.wrap.take_report(report);
            }
        }
    }

    /// Each row's start, line and whether it continues.
    fn rows(&self) -> Vec<(usize, usize, bool)> {
        shape(self.wrap.snapshot(), &self.map.snapshot())
    }

    fn fresh(&self, columns: usize) -> Vec<(usize, usize, bool)> {
        let display = self.map.snapshot();
        shape(&WrapSnapshot::new(&display, columns), &display)
    }
}

fn shape(wrap: &WrapSnapshot, display: &DisplaySnapshot) -> Vec<(usize, usize, bool)> {
    (0..wrap.row_count())
        .map(|index| {
            let row = wrap.row(display, index);
            (row.start, row.line, row.continues)
        })
        .collect()
}

#[test]
fn a_command_past_its_budget_leaves_the_rest_to_a_job() {
    let text = "日本語の文章です。".repeat(8);
    let mut window = Window::new(&text, 9);
    window.command(&[(0, 0, "あ")]);
    assert!(
        window.wrap.is_stale(),
        "the budget cannot reach the line's end"
    );
    window.settle();
    assert!(!window.wrap.is_stale());
    assert_eq!(window.rows(), window.fresh(9));
}

#[test]
fn a_newer_width_supersedes_a_job_and_its_report_is_dropped() {
    let mut window = Window::new(&"word ".repeat(40), 30);
    window.wrap.set_columns(20);
    let first = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    window.wrap.set_columns(12);
    let second = window
        .wrap
        .take_job(&window.map.snapshot())
        .expect("a newer job");
    assert!(first.run().is_none(), "superseded, it stops");
    let report = second.run().expect("a report");
    assert!(window.wrap.take_report(report));
    assert_eq!(window.rows(), window.fresh(12));
}

#[test]
fn a_report_takes_the_commands_made_while_its_job_ran() {
    let mut window = Window::new(&"word ".repeat(40), 30);
    window.wrap.set_columns(16);
    let job = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    window.command(&[(10, 0, "longer ")]);
    window.command(&[(0, 5, "")]);
    assert!(window.wrap.take_report(job.run().expect("a report")));
    window.settle();
    assert_eq!(window.rows(), window.fresh(16));
}

#[test]
fn a_report_past_what_is_kept_since_runs_its_job_again() {
    let mut window = Window::new(&"word ".repeat(40), 30);
    window.wrap.set_columns(16);
    let job = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    for _ in 0..=SINCE_COMMANDS {
        window.command(&[(0, 0, "x")]);
    }
    assert!(!window.wrap.take_report(job.run().expect("a report")));
    assert!(window.wrap.is_stale(), "it wants the width wrapped again");
    window.settle();
    assert_eq!(window.rows(), window.fresh(16));
}

/// Text with long lines of words and of CJK.
fn text() -> impl Strategy<Value = String> {
    let pieces = prop::sample::select(
        &[
            "word ",
            "ab",
            " ",
            "日本語",
            "。",
            "「",
            "」",
            "\t",
            "\n",
            "abcdefghij",
        ][..],
    );
    prop::collection::vec(pieces, 0..60).prop_map(|pieces| pieces.concat())
}

proptest! {
    /// Whatever commands, widths and jobs interleave, once nothing is stale the rows are
    /// those of a wrap from scratch.
    #[test]
    fn once_settled_the_rows_are_a_wrap_from_scratch(
        text in text(),
        steps in prop::collection::vec(
            (0_usize..4, prop::collection::vec((0_usize..300, 0_usize..4, text()), 1..4), 8_usize..20),
            1..8,
        ),
        columns in 8_usize..20,
    ) {
        let mut window = Window::new(&text, columns);
        let mut columns = columns;
        let mut held = None;
        for (kind, replacements, width) in steps {
            match kind {
                0 => {
                    columns = width;
                    window.wrap.set_columns(width);
                }
                1 => held = window.wrap.take_job(&window.map.snapshot()).or(held),
                2 => {
                    if let Some(report) = held.take().and_then(WrapJob::run) {
                        window.wrap.take_report(report);
                    }
                }
                _ => {}
            }
            let mut replacements: Vec<(usize, usize, &str)> = replacements
                .iter()
                .map(|(start, len, text)| (*start, *len, text.as_str()))
                .collect();
            replacements.sort_by_key(|&(start, ..)| std::cmp::Reverse(start));
            window.command(&replacements);
        }
        if let Some(report) = held.take().and_then(WrapJob::run) {
            window.wrap.take_report(report);
        }
        window.settle();
        prop_assert_eq!(window.rows(), window.fresh(columns));
    }
}
