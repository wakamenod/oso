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

    /// A window that has just turned wrapping on: its rows provisional.
    fn provisional(text: &str, columns: usize) -> Self {
        let rope = Rope::from(text);
        let map = DisplayMap::new(&rope, TabWidth::new(4));
        let wrap = WrapMap::provisional(&map.snapshot(), columns);
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
        self.wrap.refill();
        self.wrap.edit(&splices);
    }

    /// Runs jobs until nothing is stale.
    fn settle(&mut self) {
        while let Some(job) = self.wrap.take_job(&self.map.snapshot()) {
            if let Some(report) = job.run() {
                self.wrap.take_report(report, &self.map.snapshot());
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
fn a_report_from_another_maps_job_is_not_taken() {
    let mut one = Window::new(&"word ".repeat(40), 30);
    let mut other = Window::new(&"word ".repeat(40), 30);
    one.wrap.set_columns(20, &one.map.snapshot());
    other.wrap.set_columns(20, &other.map.snapshot());
    let job = one.wrap.take_job(&one.map.snapshot()).expect("a job");
    let _theirs = other.wrap.take_job(&other.map.snapshot()).expect("a job");
    let report = job.run().expect("a report");
    assert!(
        !other.wrap.owns(&report),
        "of the same generation, but not theirs"
    );
    assert!(!other.wrap.take_report(report, &other.map.snapshot()));
}

#[test]
fn a_job_stops_once_its_map_is_gone() {
    let mut window = Window::new(&"word ".repeat(40), 30);
    window.wrap.set_columns(20, &window.map.snapshot());
    let job = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    let clone = window.wrap.clone();
    drop(window);
    let job_of_clone = {
        let mut clone = clone;
        clone.set_columns(
            12,
            &DisplayMap::new(&Rope::from("x"), TabWidth::new(4)).snapshot(),
        );
        clone.take_job(&DisplayMap::new(&Rope::from("x"), TabWidth::new(4)).snapshot())
    };
    assert!(
        job.run().is_none(),
        "superseded through the clone, then gone"
    );
    assert!(
        job_of_clone.expect("a job").run().is_none(),
        "its map is gone"
    );
}

#[test]
fn provisional_rows_near_the_view_wrap_at_once_and_the_rest_wait() {
    let line = "word ".repeat(60);
    let text = format!("{line}\n{line}\n{line}\n");
    let mut window = Window::provisional(&text, 12);
    assert_eq!(
        window.rows().len(),
        4,
        "a row a line, and the empty last one"
    );
    let display = window.map.snapshot();
    let at = line.len() + 1 + 150;
    window.wrap.refill();
    assert!(window.wrap.realize(&display, at, 0));
    assert_eq!(window.wrap.snapshot().len(), display.len());
    let snapshot = window.wrap.snapshot();
    let row = snapshot.row_span(&display, snapshot.row_of(&display, at));
    assert!(row.1 - row.0 <= 12, "the row that holds it is wrapped");
    assert!(window.wrap.is_stale(), "the rest waits for the job");
    window.wrap.refill();
    assert!(
        !window.wrap.realize(&display, at, 0),
        "and is not wrapped again"
    );
    window.settle();
    assert_eq!(window.rows(), window.fresh(12));
}

#[test]
fn a_reset_drops_the_running_jobs_report_and_its_next_job_supersedes_it() {
    let mut window = Window::new(&"word ".repeat(40), 30);
    window.wrap.set_columns(20, &window.map.snapshot());
    let job = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    window.wrap.reset(&window.map.snapshot());
    let report = job.run().expect("not yet superseded");
    assert!(!window.wrap.take_report(report, &window.map.snapshot()));
    let held = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    let next = window.wrap.take_job(&window.map.snapshot());
    assert!(next.is_none(), "one at a time");
    let report = held.run().expect("a report");
    assert!(window.wrap.take_report(report, &window.map.snapshot()));
    assert_eq!(window.rows(), window.fresh(20));
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
    window.wrap.set_columns(20, &window.map.snapshot());
    let first = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    window.wrap.set_columns(12, &window.map.snapshot());
    let second = window
        .wrap
        .take_job(&window.map.snapshot())
        .expect("a newer job");
    assert!(first.run().is_none(), "superseded, it stops");
    let report = second.run().expect("a report");
    assert!(window.wrap.take_report(report, &window.map.snapshot()));
    assert_eq!(window.rows(), window.fresh(12));
}

#[test]
fn a_report_takes_the_commands_made_while_its_job_ran() {
    let mut window = Window::new(&"word ".repeat(40), 30);
    window.wrap.set_columns(16, &window.map.snapshot());
    let job = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    window.command(&[(10, 0, "longer ")]);
    window.command(&[(0, 5, "")]);
    assert!(
        window
            .wrap
            .take_report(job.run().expect("a report"), &window.map.snapshot())
    );
    window.settle();
    assert_eq!(window.rows(), window.fresh(16));
}

#[test]
fn a_report_after_many_commands_is_taken_and_leaves_what_they_changed_stale() {
    let mut window = Window::new(&"word ".repeat(40), 30);
    window.wrap.set_columns(16, &window.map.snapshot());
    let job = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    for _ in 0..100 {
        window.command(&[(0, 0, "x")]);
    }
    let report = job.run().expect("a report");
    assert!(window.wrap.take_report(report, &window.map.snapshot()));
    assert!(
        window.wrap.is_stale(),
        "what the commands changed waits for a job"
    );
    window.settle();
    assert_eq!(window.rows(), window.fresh(16));
}

#[test]
fn spread_splices_past_the_budget_leave_only_their_own_rows_stale() {
    // Lines longer than a rope chunk, so that each splice rebuilds chunks of its own.
    let line = "word ".repeat(600);
    let text = format!("{line}\n{line}\n{line}\n{line}\n");
    let mut window = Window::new(&text, 12);
    let rows_before = window.rows().len();
    // From the end back, as the editor applies them, one a line.
    window.command(&[
        (10_000, 0, "x"),
        (7_000, 0, "x"),
        (4_000, 0, "x"),
        (1_000, 0, "x"),
    ]);
    let Some(Work::Ranges(ranges)) = &window.wrap.wanted else {
        panic!("stale ranges");
    };
    assert!(ranges.len() > 1, "apart: {}", ranges.len());
    let stale: usize = ranges.iter().map(|range| range.len()).sum();
    assert!(
        stale < text.len() / 2,
        "each small: {stale} of {}",
        text.len()
    );
    // Each splice's rebuilt chunks are one provisional row; the rows between stay wrapped.
    assert!(
        window.rows().len() * 2 > rows_before,
        "{} of {rows_before}",
        window.rows().len()
    );
    window.settle();
    assert_eq!(window.rows(), window.fresh(12));
}

#[test]
fn a_command_that_empties_the_buffer_leaves_no_row_behind() {
    let mut window = Window::new("abcdefghijklmnop", 9);
    window.command(&[(0, 16, ""), (0, 0, "y"), (0, 0, "x")]);
    assert_eq!(window.rows(), window.fresh(9));
    window.settle();
    assert_eq!(window.rows(), window.fresh(9));
}

#[test]
fn far_apart_places_typed_in_during_a_job_stay_apart_once_it_reports() {
    let line = "word ".repeat(600);
    let text = format!("{line}\n{line}\n{line}\n{line}\n");
    let mut window = Window::new(&text, 30);
    window.wrap.set_columns(12, &window.map.snapshot());
    let job = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    window.command(&[(10_000, 0, "x")]);
    window.command(&[(1_000, 0, "x")]);
    let report = job.run().expect("a report");
    assert!(window.wrap.take_report(report, &window.map.snapshot()));
    let Some(Work::Ranges(ranges)) = &window.wrap.wanted else {
        panic!("stale ranges");
    };
    let stale: usize = ranges.iter().map(|range| range.len()).sum();
    assert!(
        ranges.len() >= 2 && stale < text.len() / 2,
        "{} ranges, {stale} bytes",
        ranges.len()
    );
    window.settle();
    assert_eq!(window.rows(), window.fresh(12));
}

/// Found by the review: a lower place's join reaching into the rows an upper one already
/// replaced, with no empty edit.
#[test]
fn a_lower_join_reaching_an_upper_one_joins_with_it() {
    let mut window = Window::new(
        "日本語word ab日本語ab 。abword abcdefghijabcdefghij abab",
        16,
    );
    window.command(&[(19, 0, "\n")]);
    let job = window.wrap.take_job(&window.map.snapshot()).expect("a job");
    window.command(&[(29, 1, "")]);
    window.command(&[(34, 0, "x")]);
    let report = job.run().expect("a report");
    assert!(window.wrap.take_report(report, &window.map.snapshot()));
    let display = window.map.snapshot();
    let mut at = 0;
    for index in 0..window.wrap.snapshot().row_count() {
        let row = window.wrap.snapshot().row(&display, index);
        assert_eq!(row.start, at, "rows tile");
        at = row.end;
    }
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
            (0_usize..5, prop::collection::vec((0_usize..300, 0_usize..4, text()), 1..4), 4_usize..20),
            1..8,
        ),
        columns in 4_usize..20,
        provisional in any::<bool>(),
    ) {
        let mut window = if provisional {
            Window::provisional(&text, columns)
        } else {
            Window::new(&text, columns)
        };
        let mut columns = columns;
        let mut held = None;
        for (kind, replacements, width) in steps {
            match kind {
                0 => {
                    columns = width;
                    window.wrap.set_columns(width, &window.map.snapshot());
                }
                1 => held = window.wrap.take_job(&window.map.snapshot()).or(held),
                2 => {
                    if let Some(report) = held.take().and_then(WrapJob::run) {
                        window.wrap.take_report(report, &window.map.snapshot());
                    }
                }
                3 => {
                    let display = window.map.snapshot();
                    let at = replacements[0].0 % (display.len() + 1);
                    window.wrap.refill();
                    window.wrap.realize(&display, at, width % 5);
                    prop_assert_eq!(window.wrap.snapshot().len(), display.len(), "rows cover the text");
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
            window.wrap.take_report(report, &window.map.snapshot());
        }
        window.settle();
        prop_assert_eq!(window.rows(), window.fresh(columns));
    }
}

/// Found by the window integration: below the narrowest width nothing wraps, so a row is a
/// whole line, and a report's places on one line must join as one.
#[test]
fn places_on_one_unwrapped_line_join_as_one() {
    let text = "日本語word ababword word  word word word word abcdefghijword word word word ababab";
    let rope = Rope::from(text);
    let map = DisplayMap::new(&rope, TabWidth::new(4));
    let wrap = WrapMap::new(&map.snapshot(), 4);
    let mut window = Window { rope, map, wrap };
    window.command(&[(3, 0, "。。word ab日本語abcdefghij。")]);
    window.command(&[(0, 0, "。。abab\n\t 。\tword 。。。日本語。。\nword word word 日本語word 日本語word word word  abcdefghij abab\t")]);
    let held = window.wrap.take_job(&window.map.snapshot());
    window.command(&[(120, 0, "")]);
    let held = window.wrap.take_job(&window.map.snapshot()).or(held);
    window.command(&[
        (105, 0, "\nabcdefghij日本語abcdefghijword\n\nabword日本語"),
        (0, 0, ""),
    ]);
    if let Some(report) = held.and_then(WrapJob::run) {
        window.wrap.take_report(report, &window.map.snapshot());
    }
    window.settle();
    assert_eq!(window.rows(), window.fresh(4));
}
