//! Tests of highlighting's glue: secrets first (ADR 0111), then what jobs carry, reports and
//! failures, and two real threads with a slow buffer beside a shown one.
//!
//! Must not depend on: the event loop; these call the glue and the pool directly.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use oso_core::buffer::{Buffer, BufferId, Replacement, Verdict};
use oso_core::display_map::TabWidth;
use oso_core::editor::Editor;
use oso_core::load::decode_all;
use oso_core::rope::Rope;
use oso_core::syntax::{Highlight, Highlights};
use oso_core::window::ViewSize;
use oso_syntax::{Failure, Job, Language, Outcome, Refused, Report};

use super::pool::Pool;
use super::{MOST_RUNS_COMPARED, Orders, Syntax, Taken, changed};

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
    assert!(Syntax::default().orders(&editor).jobs.is_empty());
}

#[test]
fn a_buffer_that_may_hold_secrets_gets_no_job() {
    let (editor, _) = editor_with(visited("/p/.env.toml", "a = 1\n"));
    assert!(Syntax::default().orders(&editor).jobs.is_empty());
}

#[test]
fn a_buffer_that_turns_secret_is_dropped_before_any_job() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
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
    let mut syntax = Syntax::default();
    assert_eq!(job_ids(&syntax.orders(&editor)), [id]);
    editor.kill_buffer(id);
    assert_eq!(syntax.drops(&editor), [id]);
}

#[test]
fn a_job_carries_the_edits_since_the_last_and_what_windows_show() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", &"fn a() {}\n".repeat(50)));
    let mut syntax = Syntax::default();
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
    editor.scroll_view(editor.tiled().0.id(), 20);
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
    assert!(Syntax::default().orders(&editor).jobs.is_empty());
}

#[test]
fn a_report_is_taken_and_a_failure_stops_the_buffer_and_names_only_it() {
    let (mut editor, id) = editor_with(visited("/p/secret-plan/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
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
    let shown = Taken {
        redraw: true,
        message: None,
    };
    assert_eq!(syntax.take(&mut editor, report), shown);
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.highlights().unwrap().at(0), Some(Highlight::Keyword));
    let failed = Report {
        buffer: id,
        epoch: job.epoch,
        stamp: 2,
        revision: job.revision,
        outcome: Outcome::Failed(Failure::TooSlow),
    };
    let message = syntax.take(&mut editor, failed).message.unwrap();
    assert_eq!(message, "Highlighting stopped in main.rs: it took too long");
    assert!(!message.contains("secret-plan") && !message.contains("fn a"));
    assert!(editor.buffer(id).unwrap().highlights().is_none());
    edit(&mut editor, id, 0, "x");
    assert!(syntax.orders(&editor).jobs.is_empty(), "it stays plain");
}

#[test]
fn a_report_for_a_dropped_buffer_is_ignored() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
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
    assert_eq!(syntax.take(&mut editor, report), Taken::default());
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
    let mut syntax = Syntax::default();
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
    let failed = Report {
        buffer: background,
        epoch: syntax.sent[&background].epoch,
        stamp: u64::MAX,
        revision: 0,
        outcome: Outcome::Failed(Failure::TooSlow),
    };
    let message = syntax.take(&mut editor, failed).message;
    assert_eq!(
        message.as_deref(),
        Some("Highlighting stopped in slow.rs: it took too long")
    );
}

/// A key as the review's (ADR 0122 step 9): buffers, and documents no buffer owns, apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    Buffer(BufferId),
    Document(u64),
}

/// Two real threads, with documents no buffer owns beside a shown buffer: a crafted document
/// that fails as too slow holds the one thread documents may have, never the shown buffer's,
/// and the other document waits for it. The order inside the lane is tested in `oso_syntax`.
#[test]
fn documents_share_the_threads_and_a_slow_one_never_holds_a_shown_buffer_back() {
    let slow = "(".repeat(64 << 10);
    let (editor, shown) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let (reports, received) = mpsc::channel();
    let pool = Pool::start(move |report| drop(reports.send(report))).unwrap();
    let open = |key, verdict, text: &str, shown| {
        pool.open(
            Key::Document(key),
            Language::Rust,
            verdict,
            Rope::from(text),
            shown,
        )
    };
    open(
        1,
        Verdict::Ordinary,
        &slow,
        std::iter::once(0..slow.len()).collect(),
    )
    .unwrap();
    open(2, Verdict::Ordinary, "fn b() {}\n", Vec::new()).unwrap();
    let secret = open(3, Verdict::SecretName, "fn c() {}\n", Vec::new());
    assert_eq!(secret, Err(Refused::MayHoldSecrets));
    let jobs = (Syntax::default().orders(&editor).jobs.into_iter())
        .map(|job| Job {
            buffer: Key::Buffer(job.buffer),
            language: job.language,
            epoch: job.epoch,
            revision: job.revision,
            text: job.text,
            edits: job.edits,
            shown: job.shown,
        })
        .collect();
    pool.send(Orders {
        drops: Vec::new(),
        jobs,
    });
    let mut order = Vec::new();
    while order.len() < 3 {
        let report = received.recv_timeout(Duration::from_secs(30)).unwrap();
        match (report.buffer, report.outcome) {
            (Key::Buffer(id), Outcome::Highlights(_)) if id == shown => order.push("buffer"),
            (Key::Document(2), Outcome::Highlights(_)) => order.push("document"),
            (Key::Document(1), Outcome::Failed(Failure::TooSlow)) => order.push("slow"),
            (_, Outcome::Highlights(_)) => {}
            (key, outcome) => panic!("{key:?}: {outcome:?}"),
        }
        order.dedup();
    }
    assert_eq!(order, ["buffer", "slow", "document"]);
    let late = received.recv_timeout(Duration::from_millis(100));
    assert!(late.is_err(), "a report after the end: {late:?}");
    // Done, and closed: nothing more comes of it.
    pool.show(Key::Document(2), std::iter::once(0..4).collect());
    pool.close(Key::Document(2));
}

#[test]
fn highlighting_runs_in_every_session() {
    let (editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    assert_eq!(job_ids(&Syntax::default().orders(&editor)), [id]);
}

/// A report asks for a frame only where a window shows runs it changed (ADR 0111).
#[test]
fn a_report_asks_for_a_frame_only_if_a_shown_run_changed() {
    let text = "fn a() {}\n".repeat(40);
    let (mut editor, id) = editor_with(visited("/p/main.rs", &text));
    let mut syntax = Syntax::default();
    let job = syntax.orders(&editor).jobs.remove(0);
    let mut stamp = 0;
    // A report with a keyword at each of `starts`.
    let mut take = |editor: &mut Editor, starts: &[usize]| {
        let mut highlights = Highlights::new(job.text.len());
        for &at in starts {
            highlights.replace(at..at + 2, [(2, Some(Highlight::Keyword))]);
        }
        stamp += 1;
        let report = Report {
            buffer: id,
            epoch: job.epoch,
            stamp,
            revision: job.revision,
            outcome: Outcome::Highlights(highlights),
        };
        syntax.take(editor, report).redraw
    };
    assert!(take(&mut editor, &[0]), "the first line's keyword");
    assert!(!take(&mut editor, &[0]), "the same runs again");
    // The window shows 10 of the 40 lines: a run on line 30 changes nothing it shows.
    let unshown = 30 * "fn a() {}\n".len();
    assert!(!take(&mut editor, &[0, unshown]), "a run out of view");
    assert!(
        take(&mut editor, &[0, 10, unshown]),
        "the second line's keyword"
    );
}

#[test]
fn a_report_from_before_a_new_start_or_older_than_one_taken_is_dropped() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
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
        Taken::default(),
        "a stale failure stopped it"
    );
    assert!(syntax.sent.contains_key(&id));
}

#[test]
fn font_lock_off_drops_every_buffer_and_sends_no_job() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
    assert_eq!(job_ids(&syntax.orders(&editor)), [id]);
    editor.set_font_lock(false);
    assert_eq!(syntax.drops(&editor), [id]);
    edit(&mut editor, id, 0, "x");
    assert!(syntax.orders(&editor).is_empty());
    editor.set_font_lock(true);
    let again = syntax.orders(&editor);
    assert_eq!(again.jobs[0].edits, None, "back on, it starts from nothing");
}

#[test]
fn setting_the_mode_again_gives_a_stopped_buffer_another_try() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
    let job = syntax.orders(&editor).jobs.remove(0);
    let failed = Report {
        buffer: id,
        epoch: job.epoch,
        stamp: 1,
        revision: job.revision,
        outcome: Outcome::Failed(Failure::TooSlow),
    };
    assert!(syntax.take(&mut editor, failed).message.is_some());
    assert!(syntax.orders(&editor).jobs.is_empty(), "stopped");
    editor
        .buffer_mut(id)
        .unwrap()
        .set_mode(oso_core::buffer::Mode::Rust);
    assert!(
        syntax.orders(&editor).jobs.is_empty(),
        "too soon after it stopped"
    );
    // Later, the same mode set again tries again.
    let long_ago = Instant::now().checked_sub(super::RETRY_EVERY * 2).unwrap();
    syntax.stopped.get_mut(&id).unwrap().1 = long_ago;
    let retry = syntax.orders(&editor);
    assert_eq!(job_ids(&retry), [id], "rust-mode again tries again");
    assert!(retry.jobs[0].epoch > job.epoch, "from a new start");
}

#[test]
fn two_mode_changes_in_one_command_start_over_though_they_end_where_they_began() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
    let first = syntax.orders(&editor).jobs.remove(0);
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.set_mode(oso_core::buffer::Mode::Markdown);
    buffer.set_mode(oso_core::buffer::Mode::Rust);
    let again = syntax.orders(&editor);
    assert_eq!(
        job_ids(&again),
        [id],
        "the mode was set, and the highlights forgotten"
    );
    assert_eq!(again.jobs[0].edits, None);
    assert!(again.jobs[0].epoch > first.epoch);
}

/// A shown range of more runs than a report's comparison steps over counts as changed, so that
/// long lines shown whole cost a frame rather than a walk of them all.
#[test]
fn a_comparison_stops_after_its_most_runs_and_counts_as_changed() {
    let runs = |count: usize| {
        let mut highlights = Highlights::new(2 * count);
        for at in 0..count {
            highlights.replace(2 * at..2 * at + 1, [(1, Some(Highlight::Keyword))]);
        }
        highlights
    };
    let few = runs(MOST_RUNS_COMPARED / 4);
    assert!(!changed(Some(&few), Some(&few.clone()), &(0..few.len())));
    let many = runs(MOST_RUNS_COMPARED);
    assert!(changed(Some(&many), Some(&many.clone()), &(0..many.len())));
    assert!(!changed(Some(&many), Some(&many.clone()), &(0..2)));
}

/// The switch-buffer preview shows a buffer no window does: a report for it draws a frame.
#[test]
fn a_report_for_a_buffer_only_previewed_asks_for_a_frame() {
    use oso_core::complete::Candidate;
    use oso_core::editor::CompletionStyle;
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
    let job = syntax.orders(&editor).jobs.remove(0);
    let scratch = editor.buffer_named("*scratch*").unwrap();
    editor.show(scratch);
    let style = CompletionStyle {
        given_order: true,
        takes_input: true,
        preview: true,
    };
    let names = [Candidate::new("main.rs", "", "")];
    let switch = editor.open_completion_as("Switch to buffer:", "buffer", names.into(), style);
    switch.unwrap();
    let mut highlights = Highlights::new(job.text.len());
    highlights.replace(0..2, [(2, Some(Highlight::Keyword))]);
    let report = Report {
        buffer: id,
        epoch: job.epoch,
        stamp: 1,
        revision: job.revision,
        outcome: Outcome::Highlights(highlights),
    };
    assert!(syntax.take(&mut editor, report).redraw);
}
