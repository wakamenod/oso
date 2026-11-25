//! Tests of highlighting the sides a review shows read-only (ADR 0122, step 3): the only
//! internal buffers highlighted besides a description at point.
//!
//! Must not depend on: the event loop; these call the glue directly.

use oso_core::buffer::Mode;
use oso_core::syntax::{Highlight, Highlights};
use oso_syntax::{Failure, Language, Outcome, Report};

use super::Syntax;
use super::tests::{editor_with, job_ids, visited};

#[test]
fn a_review_s_side_gets_a_job_while_a_window_shows_it_and_takes_its_report() {
    let (mut editor, id) = editor_with(visited("/w/p/main.rs", "fn main() {}\n"));
    let side = editor.add_side_buffer("*new: b.rs \u{b7} commit abc*", Mode::Rust);
    let text = oso_core::load::decode_all(b"fn b() {}\n", Default::default())
        .compared_with(&oso_core::rope::Rope::new());
    let _ = editor.set_internal_reloaded(side, text);
    let mut syntax = Syntax::default();
    assert!(
        !job_ids(&syntax.orders(&editor)).contains(&side),
        "no window"
    );

    let window = editor.window().id();
    editor.show_in(window, side);
    let orders = syntax.orders(&editor);
    let job = (orders.jobs.iter().find(|job| job.buffer == side)).expect("a job");
    assert_eq!(job.language, Language::Rust);
    assert!(syntax.orders(&editor).jobs.is_empty(), "sent once");
    let mut highlights = Highlights::new(job.text.len());
    highlights.replace(0..2, [(2, Some(Highlight::Keyword))]);
    let report = Report {
        buffer: side,
        epoch: job.epoch,
        stamp: 1,
        revision: job.revision,
        outcome: Outcome::Highlights(highlights),
        done: true,
    };
    assert!(syntax.take(&mut editor, report).redraw);
    assert!(editor.buffer(side).unwrap().highlights().is_some());

    // Another side put in it starts from nothing, and none of the old runs show.
    let text = oso_core::load::decode_all(b"struct S;\n", Default::default())
        .compared_with(&oso_core::rope::Rope::new());
    let _ = editor.set_internal_reloaded(side, text);
    let orders = syntax.orders(&editor);
    let job = (orders.jobs.iter().find(|job| job.buffer == side)).expect("again");
    assert!(job.edits.is_none());

    // Off while font-lock is off, and dropped once no window shows it.
    editor.set_font_lock(false);
    assert!(editor.buffer(side).unwrap().highlights().is_none());
    assert!(syntax.drops(&editor).contains(&side));
    editor.set_font_lock(true);
    assert!(job_ids(&syntax.orders(&editor)).contains(&side));
    editor.show_in(window, id);
    assert!(syntax.drops(&editor).contains(&side));
}

#[test]
fn an_internal_buffer_of_a_language_is_highlighted_by_no_one_but_a_side() {
    let (mut editor, _) = editor_with(visited("/w/p/main.rs", "fn main() {}\n"));
    let other = editor.add_internal_buffer("*Report*", Mode::Rust);
    editor.set_internal_text(other, "fn a() {}\n");
    let window = editor.window().id();
    editor.show_in(window, other);
    assert!(!job_ids(&Syntax::default().orders(&editor)).contains(&other));
}

#[test]
fn a_side_whose_highlighting_failed_is_not_tried_again_by_hiding_it() {
    let (mut editor, id) = editor_with(visited("/w/p/main.rs", "fn main() {}\n"));
    let side = editor.add_side_buffer("*new: b.rs \u{b7} commit abc*", Mode::Rust);
    editor.set_internal_text(side, "fn b() {}\n");
    let window = editor.window().id();
    editor.show_in(window, side);
    let mut syntax = Syntax::default();
    let orders = syntax.orders(&editor);
    let job = (orders.jobs.iter().find(|job| job.buffer == side)).expect("a job");
    let report = Report {
        buffer: side,
        epoch: job.epoch,
        stamp: 1,
        revision: job.revision,
        outcome: Outcome::Failed(Failure::TooSlow),
        done: true,
    };
    assert!(syntax.take(&mut editor, report).message.is_some());
    editor.show_in(window, id);
    syntax.drops(&editor);
    editor.show_in(window, side);
    assert!(!job_ids(&syntax.orders(&editor)).contains(&side));
}

#[test]
fn a_side_two_windows_show_gets_one_job() {
    let (mut editor, _) = editor_with(visited("/w/p/main.rs", "fn main() {}\n"));
    editor.set_frame_size(oso_core::frame::FrameSize {
        rows: 30,
        columns: 100,
    });
    let side = editor.add_side_buffer("*new: b.rs \u{b7} commit abc*", Mode::Rust);
    editor.set_internal_text(side, "fn b() {}\n");
    let first = editor.window().id();
    editor
        .split_window(oso_core::frame::Way::Down, true)
        .unwrap();
    let second = editor.window().id();
    assert_ne!(first, second);
    editor.show_in(first, side);
    editor.show_in(second, side);
    let orders = Syntax::default().orders(&editor);
    let jobs = job_ids(&orders).iter().filter(|id| **id == side).count();
    assert_eq!(jobs, 1);
}
