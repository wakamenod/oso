//! Tests of documents no buffer owns in the schedule: what is refused, the secret verdicts
//! first; closing at any point; the lane and the order they share with buffers; the caps.
//!
//! Must not depend on: threads; these drive the schedule by hand.

use std::time::Duration;

use oso_core::buffer::Verdict;
use oso_core::rope::Rope;

use super::super::{Checkout, Job, Outcome, Report, Scheduler};
use super::{MAX_DOCUMENTS, MAX_STARTED, Refused};
use crate::document::{Document, Highlighted, MAX_BYTES};
use crate::language::Language;
use crate::queries::Queries;

const LONG: Duration = Duration::from_secs(30);

const TEXT: &str = "/// A comment.\nfn a() -> &'static str { \"text\" }\n";

fn open(scheduler: &mut Scheduler<u32>, key: u32, shown: bool) {
    let text = Rope::from(TEXT);
    let shown = if shown {
        std::iter::once(0..text.len()).collect()
    } else {
        Vec::new()
    };
    scheduler
        .open(key, Language::Rust, Verdict::Ordinary, text, shown)
        .unwrap();
}

fn buffer(buffer: u32, shown: bool) -> Job<u32> {
    let text = Rope::from(TEXT);
    Job {
        buffer,
        language: Language::Rust,
        epoch: 1,
        revision: 1,
        shown: if shown {
            std::iter::once(0..text.len()).collect()
        } else {
            Vec::new()
        },
        text,
        edits: None,
    }
}

/// Takes the next document, makes its tree's document without parsing, and gives it back
/// with work left, so that it holds a tree.
fn start(scheduler: &mut Scheduler<u32>) -> u32 {
    let mut checkout: Checkout<u32> = scheduler.take_next().unwrap();
    checkout.follow();
    let key = checkout.buffer;
    assert!(scheduler.give_back(checkout, Highlighted::Paused).is_none());
    key
}

fn drain(scheduler: &mut Scheduler<u32>, queries: &mut Queries) -> Vec<Report<u32>> {
    let mut reports = Vec::new();
    while let Some(mut checkout) = scheduler.take_next() {
        let outcome = checkout.run_for(queries, LONG);
        reports.extend(scheduler.give_back(checkout, outcome));
    }
    reports
}

#[test]
fn a_text_that_may_hold_secrets_is_never_opened() {
    let mut scheduler = Scheduler::new();
    for verdict in [
        Verdict::Unjudged,
        Verdict::Unresolved,
        Verdict::SecretName,
        Verdict::Passphrase,
        Verdict::Yanked,
    ] {
        let opened = scheduler.open(1, Language::Rust, verdict, Rope::from(TEXT), Vec::new());
        assert_eq!(opened, Err(Refused::MayHoldSecrets), "{verdict:?}");
        assert!(!scheduler.has(1), "{verdict:?} kept a slot");
    }
    assert!(scheduler.take_next().is_none());
}

#[test]
fn a_text_past_the_size_limit_is_never_opened() {
    let mut scheduler = Scheduler::new();
    let text = Rope::from("a".repeat(MAX_BYTES + 1).as_str());
    let opened = scheduler.open(1, Language::Rust, Verdict::Ordinary, text, Vec::new());
    assert_eq!(opened, Err(Refused::TooLarge));
    assert!(!scheduler.has(1));
    let text = Rope::from("a".repeat(MAX_BYTES).as_str());
    assert!(
        (scheduler.open(1, Language::Rust, Verdict::Ordinary, text, Vec::new())).is_ok(),
        "the limit itself is allowed"
    );
}

#[test]
fn past_the_most_open_documents_another_waits_for_one_to_close() {
    let mut scheduler = Scheduler::new();
    for key in 0..u32::try_from(MAX_DOCUMENTS).unwrap() {
        open(&mut scheduler, key, false);
    }
    scheduler.submit(buffer(5000, false));
    let more = scheduler.open(
        5001,
        Language::Rust,
        Verdict::Ordinary,
        Rope::new(),
        Vec::new(),
    );
    assert_eq!(more, Err(Refused::Full), "buffers do not count");
    assert!(!scheduler.has(5001));
    let again = scheduler.open(
        7,
        Language::Toml,
        Verdict::Ordinary,
        Rope::new(),
        Vec::new(),
    );
    assert!(again.is_ok(), "an open document's key is replaced");
    scheduler.close(3);
    open(&mut scheduler, 5001, false);
}

#[test]
fn a_document_is_highlighted_whole_as_a_fresh_one_and_reports_epoch_and_revision_zero() {
    let mut scheduler = Scheduler::new();
    let mut queries = Queries::new();
    open(&mut scheduler, 1, false);
    let reports = drain(&mut scheduler, &mut queries);
    let mut fresh = Document::new(Language::Rust, Rope::from(TEXT));
    assert_eq!(fresh.highlight(&mut queries, &[], LONG), Highlighted::Done);
    let [report] = reports.as_slice() else {
        panic!("{} reports", reports.len());
    };
    assert_eq!((report.buffer, report.epoch, report.revision), (1, 0, 0));
    let Outcome::Highlights(highlights) = &report.outcome else {
        panic!("{:?}", report.outcome);
    };
    let runs: Vec<_> = highlights.runs(0..TEXT.len()).collect();
    assert_eq!(
        runs,
        fresh.highlights().runs(0..TEXT.len()).collect::<Vec<_>>()
    );
    assert!(runs.iter().any(|(_, highlight)| highlight.is_some()));
}

#[test]
fn a_finished_document_gives_its_tree_to_free_and_its_slot_holds_nothing() {
    let mut scheduler = Scheduler::new();
    let mut queries = Queries::new();
    open(&mut scheduler, 1, true);
    drain(&mut scheduler, &mut queries);
    assert_eq!(scheduler.take_doomed().len(), 1, "the finished tree");
    let slot = &scheduler.slots[&1];
    assert!(slot.document.is_none() && slot.queued.is_none() && slot.shown.is_empty());
    assert!(!slot.holds_tree() && !slot.would_start());
    scheduler.show(1, std::iter::once(0..4).collect());
    assert!(scheduler.slots[&1].shown.is_empty(), "shown once done");
    assert!(scheduler.take_next().is_none(), "it ran again");
    scheduler.close(1);
    assert!(!scheduler.has(1));
    assert!(scheduler.take_doomed().is_empty());
}

#[test]
fn a_document_closed_before_its_slice_never_runs() {
    let mut scheduler = Scheduler::new();
    open(&mut scheduler, 1, true);
    // Its source turned secret, or the review closed, before a thread took it.
    scheduler.close(1);
    assert!(scheduler.take_next().is_none(), "it ran");
    assert!(!scheduler.has(1));
}

#[test]
fn a_document_closed_during_its_slice_gives_back_nothing_and_waits_to_be_freed() {
    let mut scheduler = Scheduler::new();
    let mut queries = Queries::new();
    open(&mut scheduler, 1, true);
    let mut checkout = scheduler.take_next().unwrap();
    scheduler.close(1);
    open(&mut scheduler, 1, false);
    let outcome = checkout.run_for(&mut queries, LONG);
    assert!(
        scheduler.give_back(checkout, outcome).is_none(),
        "it reported"
    );
    assert_eq!(scheduler.take_doomed().len(), 1, "its tree");
    let reports = drain(&mut scheduler, &mut queries);
    assert_eq!(reports.len(), 1, "the document opened again under the key");
}

#[test]
fn close_and_show_leave_buffers_alone_and_a_job_replaces_a_document() {
    let mut scheduler = Scheduler::new();
    let mut queries = Queries::new();
    scheduler.submit(buffer(1, false));
    scheduler.close(1);
    scheduler.show(1, std::iter::once(0..4).collect());
    assert!(scheduler.has(1) && scheduler.slots[&1].shown.is_empty());
    open(&mut scheduler, 2, false);
    scheduler.submit(buffer(2, false));
    assert!(
        !scheduler.slots[&2].is_document(),
        "a job's key is a buffer's"
    );
    let reports = drain(&mut scheduler, &mut queries);
    assert!(reports.iter().all(|report| report.revision == 1));
}

#[test]
fn documents_take_one_thread_at_most_and_never_hold_a_shown_buffer_back() {
    let mut scheduler = Scheduler::new();
    open(&mut scheduler, 1, true);
    open(&mut scheduler, 2, true);
    scheduler.submit(buffer(3, false));
    let first = scheduler.take_next().unwrap();
    assert_eq!(first.buffer, 1);
    assert!(
        scheduler.take_next().is_none(),
        "a second thread on a document or a background buffer"
    );
    scheduler.submit(buffer(4, true));
    assert_eq!(
        scheduler.take_next().unwrap().buffer,
        4,
        "a shown buffer waited"
    );
    scheduler.give_back(first, Highlighted::Paused);
    assert_eq!(
        scheduler.take_next().unwrap().buffer,
        2,
        "the lane is free again"
    );
}

#[test]
fn the_lane_takes_shown_documents_then_background_buffers_then_the_other_documents() {
    let mut scheduler = Scheduler::new();
    open(&mut scheduler, 1, false);
    scheduler.submit(buffer(2, false));
    open(&mut scheduler, 3, true);
    let mut order = Vec::new();
    while let Some(checkout) = scheduler.take_next() {
        order.push(checkout.buffer);
        // Each is done, as far as the order cares.
        scheduler.give_back(checkout, Highlighted::Done);
    }
    assert_eq!(order, [3, 2, 1]);
}

#[test]
fn a_started_document_goes_before_one_not_started_and_keeps_its_place() {
    let mut scheduler = Scheduler::new();
    open(&mut scheduler, 1, false);
    open(&mut scheduler, 2, false);
    open(&mut scheduler, 3, false);
    assert_eq!(start(&mut scheduler), 1);
    assert_eq!(start(&mut scheduler), 1, "it kept its place, before 2");
    scheduler.show(3, std::iter::once(0..4).collect());
    assert_eq!(start(&mut scheduler), 3, "a shown document first");
    assert_eq!(start(&mut scheduler), 3, "and once started it stays first");
    scheduler.show(3, Vec::new());
    assert_eq!(
        start(&mut scheduler),
        1,
        "started, and in its place before 3"
    );
}

#[test]
fn at_most_four_documents_hold_a_tree_and_a_shown_one_waits_for_room() {
    let mut scheduler = Scheduler::new();
    let started = u32::try_from(MAX_STARTED).unwrap();
    for key in 0..started {
        // Shown, so that it goes before those started already.
        open(&mut scheduler, key, true);
        assert_eq!(start(&mut scheduler), key);
        scheduler.show(key, Vec::new());
    }
    open(&mut scheduler, 10, true);
    let holding = scheduler
        .slots
        .values()
        .filter(|slot| slot.holds_tree())
        .count();
    assert_eq!(holding, MAX_STARTED);
    assert_ne!(
        start(&mut scheduler),
        10,
        "a fifth tree while four hold one"
    );
    let holding = scheduler
        .slots
        .values()
        .filter(|slot| slot.holds_tree())
        .count();
    assert_eq!(holding, MAX_STARTED);
    scheduler.close(0);
    assert_eq!(start(&mut scheduler), 10, "room once one closed");
}

#[test]
fn nothing_is_parsed_or_freed_where_documents_open_show_and_close() {
    let mut scheduler = Scheduler::new();
    open(&mut scheduler, 1, false);
    assert!(
        scheduler.slots[&1].document.is_none(),
        "open made a document"
    );
    assert_eq!(start(&mut scheduler), 1);
    scheduler.show(1, std::iter::once(0..4).collect());
    open(&mut scheduler, 1, false);
    assert_eq!(
        scheduler.take_doomed().len(),
        1,
        "the replaced document waits"
    );
    assert_eq!(start(&mut scheduler), 1);
    scheduler.close(1);
    assert_eq!(
        scheduler.take_doomed().len(),
        1,
        "the closed document waits"
    );
}
