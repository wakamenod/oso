//! Tests of highlighting's glue: secrets first (ADR 0111), then what jobs carry, reports and
//! failures, and two real threads with a slow buffer beside a shown one.
//!
//! Must not depend on: the event loop; these call the glue and the pool directly.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use oso_core::buffer::{Buffer, BufferId, Replacement};
use oso_core::display_map::TabWidth;
use oso_core::editor::Editor;
use oso_core::load::decode_all;
use oso_core::syntax::{Highlight, Highlights};
use oso_core::window::ViewSize;
use oso_syntax::{Failure, Outcome, Report};

use super::pool::Pool;
use super::{Orders, Syntax};

/// A visited file at `path`, judged where it leads, holding `text`.
fn visited(path: &str, text: &str) -> Buffer {
    let path = PathBuf::from(path);
    let mut buffer = Buffer::visiting(path.clone());
    buffer.judge_secret(Some(&[path]));
    buffer.apply_load(decode_all(text.as_bytes(), TabWidth::default()));
    buffer
}

fn editor_with(buffer: Buffer) -> (Editor, BufferId) {
    let mut editor = Editor::with_buffer(Buffer::from_text("*scratch*", ""));
    let id = editor.add_buffer(buffer);
    editor.show(id);
    editor.set_view_size(ViewSize::new(10, 40));
    (editor, id)
}

fn edit(editor: &mut Editor, id: BufferId, at: usize, text: &str) {
    editor
        .apply_edits(
            id,
            &[Replacement {
                range: at..at,
                text,
            }],
        )
        .unwrap();
}

fn job_ids(orders: &Orders) -> Vec<BufferId> {
    orders.jobs.iter().map(|job| job.buffer).collect()
}

#[test]
fn a_buffer_not_judged_yet_gets_no_job() {
    let mut buffer = Buffer::visiting(PathBuf::from("/p/main.rs"));
    buffer.apply_load(decode_all(b"fn a() {}\n", TabWidth::default()));
    let (editor, _) = editor_with(buffer);
    assert!(Syntax::on().orders(&editor).jobs.is_empty());
}

#[test]
fn a_buffer_that_may_hold_secrets_gets_no_job() {
    let (editor, _) = editor_with(visited("/p/.env.toml", "a = 1\n"));
    assert!(Syntax::on().orders(&editor).jobs.is_empty());
}

#[test]
fn a_buffer_that_turns_secret_is_dropped_before_any_job() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::on();
    assert_eq!(job_ids(&syntax.orders(&editor)), [id]);
    edit(&mut editor, id, 0, "x");
    // Read again, it now leads somewhere that may hold secrets.
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.judge_secret(Some(&[PathBuf::from("/p/.env")]));
    assert!(buffer.is_secret());
    assert_eq!(
        syntax.drops(&editor),
        [id],
        "dropped at once, before the next wait"
    );
    let orders = syntax.orders(&editor);
    assert!(orders.jobs.is_empty(), "its edit is never sent");
    assert!(orders.drops.is_empty(), "dropped once");
}

#[test]
fn a_killed_buffer_is_dropped() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::on();
    assert_eq!(job_ids(&syntax.orders(&editor)), [id]);
    editor.kill_buffer(id);
    assert_eq!(syntax.drops(&editor), [id]);
}

#[test]
fn a_job_carries_the_edits_since_the_last_and_what_windows_show() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", &"fn a() {}\n".repeat(50)));
    let mut syntax = Syntax::on();
    let first = syntax.orders(&editor);
    assert_eq!(
        first.jobs[0].edits, None,
        "the first job starts from nothing"
    );
    assert_eq!(first.jobs[0].shown.len(), 1);
    assert!(
        first.jobs[0].shown[0].end < 500,
        "{:?}",
        first.jobs[0].shown
    );
    assert!(syntax.orders(&editor).jobs.is_empty(), "nothing moved");
    edit(&mut editor, id, 0, "//");
    let second = syntax.orders(&editor);
    let edits = second.jobs[0].edits.as_ref().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(
        (edits[0].start, edits[0].old_len, edits[0].new_len),
        (0, 0, 2)
    );
    editor.scroll_view(20);
    let scrolled = syntax.orders(&editor);
    assert_eq!(
        scrolled.jobs[0].edits.as_deref(),
        Some(&[][..]),
        "only the view moved"
    );
}

#[test]
fn a_plain_text_file_gets_no_job() {
    let (editor, _) = editor_with(visited("/p/notes.txt", "fn a() {}\n"));
    assert!(Syntax::on().orders(&editor).jobs.is_empty());
}

#[test]
fn a_report_is_taken_and_a_failure_stops_the_buffer_and_names_only_it() {
    let (mut editor, id) = editor_with(visited("/p/secret-plan/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::on();
    let job = syntax.orders(&editor).jobs.remove(0);
    let mut highlights = Highlights::new(job.text.len());
    highlights.replace(0..2, [(2, Some(Highlight::Keyword))]);
    let report = Report {
        buffer: id,
        epoch: job.epoch,
        stamp: 1,
        revision: job.revision,
        outcome: Outcome::Highlights(highlights),
    };
    assert_eq!(syntax.take(&mut editor, report), None);
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.highlights().unwrap().at(0), Some(Highlight::Keyword));
    let failed = Report {
        buffer: id,
        epoch: job.epoch,
        stamp: 2,
        revision: job.revision,
        outcome: Outcome::Failed(Failure::TooSlow),
    };
    let message = syntax.take(&mut editor, failed).unwrap();
    assert_eq!(message, "Highlighting stopped in main.rs: it took too long");
    assert!(!message.contains("secret-plan") && !message.contains("fn a"));
    assert!(editor.buffer(id).unwrap().highlights().is_none());
    edit(&mut editor, id, 0, "x");
    assert!(syntax.orders(&editor).jobs.is_empty(), "it stays plain");
}

#[test]
fn a_report_for_a_dropped_buffer_is_ignored() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::on();
    let job = syntax.orders(&editor).jobs.remove(0);
    editor
        .buffer_mut(id)
        .unwrap()
        .judge_secret(Some(&[PathBuf::from("/p/.env")]));
    syntax.drops(&editor);
    let report = Report {
        buffer: id,
        epoch: job.epoch,
        stamp: 1,
        revision: job.revision,
        outcome: Outcome::Highlights(Highlights::new(job.text.len())),
    };
    assert_eq!(syntax.take(&mut editor, report), None);
    assert!(editor.buffer(id).unwrap().highlights().is_none());
}

/// Two real threads end to end: a shown buffer is highlighted, and a crafted one fails as too
/// slow and reports so. That a background slice never holds a shown buffer is the schedule's
/// rule, tested in `oso_syntax` without timing.
#[test]
fn the_threads_report_highlights_and_failures() {
    // A crafted text, made of an opener repeated, that every query is slow on.
    let slow = "(".repeat(64 << 10);
    let (mut editor, shown) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let background = editor.add_buffer(visited("/p/slow.rs", &slow));
    let (reports, received) = mpsc::channel();
    let pool = Pool::start(move |report| drop(reports.send(report))).unwrap();
    let mut syntax = Syntax::on();
    pool.send(syntax.orders(&editor));
    let (mut highlighted, mut failed) = (false, false);
    while !(highlighted && failed) {
        let report = received.recv_timeout(Duration::from_secs(30)).unwrap();
        match report.outcome {
            Outcome::Highlights(_) if report.buffer == shown => highlighted = true,
            Outcome::Failed(Failure::TooSlow) if report.buffer == background => failed = true,
            Outcome::Highlights(_) => {}
            Outcome::Failed(failure) => panic!("{failure:?}"),
            Outcome::Lost => panic!("a slice panicked"),
        }
    }
    let message = syntax.take(
        &mut editor,
        Report {
            buffer: background,
            epoch: syntax.sent[&background].epoch,
            stamp: u64::MAX,
            revision: 0,
            outcome: Outcome::Failed(Failure::TooSlow),
        },
    );
    assert_eq!(
        message.as_deref(),
        Some("Highlighting stopped in slow.rs: it took too long")
    );
}

#[test]
fn highlighting_is_off_until_colour_is_drawn() {
    let (editor, _) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
    assert!(
        syntax.orders(&editor).is_empty(),
        "no job, so no thread, in normal use"
    );
}

#[test]
fn a_report_from_before_a_new_start_or_older_than_one_taken_is_dropped() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::on();
    let job = syntax.orders(&editor).jobs.remove(0);
    let report = |epoch, stamp, highlight| {
        let mut highlights = Highlights::new(job.text.len());
        highlights.replace(0..2, [(2, Some(highlight))]);
        Report {
            buffer: id,
            epoch,
            stamp,
            revision: job.revision,
            outcome: Outcome::Highlights(highlights),
        }
    };
    let at_start = |editor: &Editor| {
        editor
            .buffer(id)
            .unwrap()
            .highlights()
            .and_then(|h| h.at(0))
    };
    syntax.take(&mut editor, report(job.epoch, 5, Highlight::Keyword));
    assert_eq!(at_start(&editor), Some(Highlight::Keyword));
    // A slice that ended earlier, whose thread sent later.
    syntax.take(&mut editor, report(job.epoch, 4, Highlight::String));
    assert_eq!(
        at_start(&editor),
        Some(Highlight::Keyword),
        "an older report won"
    );
    // A failure of the document from before a new start.
    let failed = Report {
        outcome: Outcome::Failed(Failure::TooSlow),
        ..report(job.epoch - 1, 9, Highlight::String)
    };
    assert_eq!(
        syntax.take(&mut editor, failed),
        None,
        "a stale failure stopped it"
    );
    assert!(syntax.sent.contains_key(&id));
}
