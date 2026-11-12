//! Tests of the worker's schedule: dropped buffers first, as a buffer that turns secret is
//! dropped (ADR 0111); then the order, edits that wait for a slice, reports and failures.
//!
//! Must not depend on: threads; these drive the schedule by hand.

use std::time::Duration;

use oso_core::rope::Rope;
use oso_core::syntax::Splice;

use super::{Job, Outcome, Report, Scheduler};
use crate::document::{Failure, Highlighted};
use crate::language::Language;
use crate::queries::Queries;

const LONG: Duration = Duration::from_secs(30);

fn job(buffer: u32, revision: u64, text: &str, edits: Option<Vec<Splice>>) -> Job<u32> {
    Job {
        buffer,
        language: Language::Rust,
        revision,
        text: Rope::from(text),
        edits,
        shown: Vec::new(),
    }
}

fn shown(mut job: Job<u32>) -> Job<u32> {
    job.shown = std::iter::once(0..job.text.len()).collect();
    job
}

/// Runs slices of long length until nothing is left, and returns the reports.
fn drain(scheduler: &mut Scheduler<u32>, queries: &mut Queries) -> Vec<Report<u32>> {
    let mut reports = Vec::new();
    while let Some(mut checkout) = scheduler.take_next() {
        let outcome = checkout.document.highlight(queries, &checkout.shown, LONG);
        reports.extend(scheduler.give_back(checkout, outcome));
    }
    reports
}

#[test]
fn a_job_for_a_buffer_dropped_before_it_ran_never_runs() {
    let mut scheduler = Scheduler::new();
    scheduler.submit(job(1, 1, "fn a() {}\n", None));
    // The buffer turned secret, or was killed, before a thread took the job.
    scheduler.drop_buffer(1);
    assert!(scheduler.take_next().is_none(), "the queued job ran");
    assert!(!scheduler.has(1));
}

#[test]
fn edits_that_came_for_a_buffer_dropped_while_a_slice_ran_never_run() {
    let mut scheduler = Scheduler::new();
    let mut queries = Queries::new();
    scheduler.submit(job(1, 1, "fn a() {}\n", None));
    let mut checkout = scheduler.take_next().unwrap();
    // More text comes, then the buffer turns secret, while the slice runs.
    let edit = Splice {
        start: 0,
        old_len: 0,
        new_len: 3,
    };
    scheduler.submit(job(1, 2, "// fn a() {}\n", Some(vec![edit])));
    scheduler.drop_buffer(1);
    let outcome = checkout.run(&mut queries);
    assert!(
        scheduler.give_back(checkout, outcome).is_none(),
        "it reported"
    );
    assert!(scheduler.take_next().is_none(), "the edits that waited ran");
    assert!(!scheduler.has(1));
}

#[test]
fn a_buffer_dropped_and_sent_again_starts_over_and_its_old_slice_counts_for_nothing() {
    let mut scheduler = Scheduler::new();
    let mut queries = Queries::new();
    scheduler.submit(job(1, 1, "fn a() {}\n", None));
    let mut old = scheduler.take_next().unwrap();
    scheduler.drop_buffer(1);
    scheduler.submit(job(1, 5, "fn b() {}\n", None));
    let outcome = old.run(&mut queries);
    assert!(scheduler.give_back(old, outcome).is_none());
    let reports = drain(&mut scheduler, &mut queries);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].revision, 5);
}

#[test]
fn shown_buffers_go_first_and_one_thread_at_most_has_a_background_one() {
    let mut scheduler = Scheduler::new();
    scheduler.submit(job(1, 1, "fn a() {}\n", None));
    scheduler.submit(job(2, 1, "fn b() {}\n", None));
    scheduler.submit(shown(job(3, 1, "fn c() {}\n", None)));
    let first = scheduler.take_next().unwrap();
    assert_eq!(first.buffer, 3, "a shown buffer first");
    let second = scheduler.take_next().unwrap();
    assert_eq!(second.buffer, 1, "then the oldest of the others");
    assert!(
        scheduler.take_next().is_none(),
        "a second background buffer waits"
    );
    scheduler.submit(shown(job(4, 1, "fn d() {}\n", None)));
    let third = scheduler.take_next().unwrap();
    assert_eq!(
        third.buffer, 4,
        "a shown buffer does not wait for the background one"
    );
    scheduler.give_back(second, Highlighted::Paused);
    assert_eq!(
        scheduler.take_next().unwrap().buffer,
        2,
        "the background lane is free again"
    );
}

#[test]
fn edits_that_come_during_a_slice_follow_after_it() {
    let mut scheduler = Scheduler::new();
    let mut queries = Queries::new();
    scheduler.submit(shown(job(1, 1, "fn a() {}\n", None)));
    let mut checkout = scheduler.take_next().unwrap();
    let first = Splice {
        start: 0,
        old_len: 0,
        new_len: 1,
    };
    let second = Splice {
        start: 1,
        old_len: 0,
        new_len: 1,
    };
    scheduler.submit(shown(job(1, 2, "/fn a() {}\n", Some(vec![first]))));
    scheduler.submit(shown(job(1, 3, "//fn a() {}\n", Some(vec![second]))));
    assert!(
        scheduler.take_next().is_none(),
        "a checked-out buffer has one thread"
    );
    // A long slice, so that it ends, and reports, on a slow runner too.
    let outcome = checkout
        .document
        .highlight(&mut queries, &checkout.shown, LONG);
    let report = scheduler.give_back(checkout, outcome).unwrap();
    assert_eq!(
        report.revision, 1,
        "the slice's report is for the text it had"
    );
    let reports = drain(&mut scheduler, &mut queries);
    let last = reports.last().unwrap();
    assert_eq!(last.revision, 3);
    let Outcome::Highlights(highlights) = &last.outcome else {
        panic!("{:?}", last.outcome);
    };
    assert_eq!(highlights.len(), "//fn a() {}\n".len());
    assert_eq!(
        highlights.at(0),
        Some(oso_core::syntax::Highlight::Comment),
        "the edits that waited were followed"
    );
}

#[test]
fn a_job_with_nothing_new_reports_nothing() {
    let mut scheduler = Scheduler::new();
    let mut queries = Queries::new();
    scheduler.submit(shown(job(1, 1, "fn a() {}\n", None)));
    assert_eq!(drain(&mut scheduler, &mut queries).len(), 1);
    // The window scrolled: the same revision, other shown ranges.
    scheduler.submit(shown(job(1, 1, "fn a() {}\n", Some(Vec::new()))));
    assert!(drain(&mut scheduler, &mut queries).is_empty());
}

#[test]
fn a_failed_document_reports_once_and_its_slot_goes() {
    let mut scheduler = Scheduler::new();
    let mut job = job(1, 1, "x", None);
    job.language = Language::MarkdownInline;
    job.text = Rope::from("x".repeat(crate::document::MAX_BYTES).as_str());
    scheduler.submit(job);
    let reports = drain(&mut scheduler, &mut Queries::new());
    assert_eq!(reports.len(), 1);
    assert!(matches!(
        reports[0].outcome,
        Outcome::Failed(Failure::TooLarge)
    ));
    assert!(!scheduler.has(1));
}

#[test]
fn a_slice_that_panicked_loses_only_its_buffer() {
    let mut scheduler = Scheduler::new();
    scheduler.submit(job(1, 1, "fn a() {}\n", None));
    scheduler.submit(shown(job(2, 1, "fn b() {}\n", None)));
    let panicked = scheduler.take_next().unwrap();
    assert_eq!(panicked.buffer, 2);
    let report = scheduler.lost(panicked).unwrap();
    assert!(matches!(report.outcome, Outcome::Failed(Failure::Lost)));
    assert!(!scheduler.has(2));
    assert_eq!(
        drain(&mut scheduler, &mut Queries::new()).len(),
        1,
        "the other goes on"
    );
}

#[test]
fn a_new_language_starts_over() {
    let mut scheduler = Scheduler::new();
    let mut queries = Queries::new();
    scheduler.submit(job(1, 1, "a = 1\n", None));
    let checkout = scheduler.take_next().unwrap();
    let mut toml = job(1, 2, "a = 1\n", Some(Vec::new()));
    toml.language = Language::Toml;
    scheduler.submit(toml);
    assert!(scheduler.give_back(checkout, Highlighted::Paused).is_none());
    let reports = drain(&mut scheduler, &mut queries);
    assert_eq!(reports.last().unwrap().revision, 2);
}
