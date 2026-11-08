//! Scenario tests of an exit agreed to while saves run: when it goes, and when a person gets
//! the window back (ADR 0059). `App` only acts on what `saver::Leaving` says.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::time::Instant;

use super::*;
use crate::saver::tests::visited;
use crate::saver::{AfterSave, Leaving};

/// Runs each waiting save, as its thread would, one at a time, and returns what each means
/// for the exit under way.
fn finish_while_leaving(editor: &mut Editor, leaving: &mut Leaving) -> Vec<Option<AfterSave>> {
    let mut steps = Vec::new();
    for job in editor.take_save_jobs() {
        let report = saver::run(job);
        let failed = report.failed();
        saver::apply(editor, report);
        steps.push(leaving.save_ended(editor, failed));
    }
    steps
}

/// Confirms the exit with the keys, as `App::leave` would be asked to.
fn confirm_quit(dispatcher: &mut Dispatcher, editor: &mut Editor) {
    assert!(!send(dispatcher, editor, QUIT).exit, "asked first");
    assert!(send(dispatcher, editor, "y").exit, "then confirmed");
}

#[test]
fn undoing_during_a_save_takes_a_confirmed_exit_back_when_it_lands() {
    let scratch = Scratch::new("dispatch-leave-undo");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s C-z");
    assert!(!modified(&editor), "it looks unmodified");
    confirm_quit(&mut dispatcher, &mut editor);
    let mut leaving = Leaving::default();
    assert!(leaving.begin(&editor, false, Instant::now()).is_some());
    let steps = finish_while_leaving(&mut editor, &mut leaving);
    assert_eq!(steps, [Some(AfterSave::StayOpen)]);
    assert!(!leaving.is_under_way());
    assert!(modified(&editor), "the undone state is only here");
    assert_eq!(fs::read(&path).unwrap(), b"xabc\n");
}

#[test]
fn the_sockets_exit_goes_even_so() {
    let scratch = Scratch::new("dispatch-leave-socket");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s C-z");
    let mut leaving = Leaving::default();
    assert!(leaving.begin(&editor, true, Instant::now()).is_some());
    let steps = finish_while_leaving(&mut editor, &mut leaving);
    assert_eq!(steps, [Some(AfterSave::Exit)]);
}

#[test]
fn a_save_that_lands_with_only_what_the_person_agreed_to_lose_goes() {
    let scratch = Scratch::new("dispatch-leave-agreed");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    // A later edit, which the refusal counted as a modified buffer.
    send(&mut dispatcher, &mut editor, "x C-x C-s y");
    confirm_quit(&mut dispatcher, &mut editor);
    let mut leaving = Leaving::default();
    assert!(leaving.begin(&editor, false, Instant::now()).is_some());
    let steps = finish_while_leaving(&mut editor, &mut leaving);
    assert_eq!(steps, [Some(AfterSave::Exit)]);
    assert!(modified(&editor));
}

#[test]
fn a_failed_save_takes_a_confirmed_exit_back() {
    let scratch = Scratch::new("dispatch-leave-failed");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    confirm_quit(&mut dispatcher, &mut editor);
    let mut leaving = Leaving::default();
    assert!(leaving.begin(&editor, false, Instant::now()).is_some());
    fs::write(&path, b"another program\n").unwrap();
    let steps = finish_while_leaving(&mut editor, &mut leaving);
    assert_eq!(steps, [Some(AfterSave::StayOpen)]);
    assert!(modified(&editor));
}

/// An editor with two file buffers, each with a save waiting, the second shown.
fn two_saves(scratch: &Scratch) -> (Editor, Vec<oso_core::editor::SaveJob>) {
    let path = scratch.file("first.txt", b"abc\n");
    let other = scratch.file("second.txt", b"abc\n");
    let mut dispatcher = Dispatcher::new();
    let mut editor = visiting(&path);
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    let mut jobs = editor.take_save_jobs();
    let second = editor.add_buffer(visited(&other));
    editor.show(second);
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    jobs.extend(editor.take_save_jobs());
    (editor, jobs)
}

#[test]
fn past_the_limit_a_person_is_back_and_the_next_exit_skips_only_the_stuck_save() {
    let scratch = Scratch::new("dispatch-leave-stuck");
    let (mut editor, mut jobs) = two_saves(&scratch);
    let second = jobs.pop().unwrap();
    let first = jobs.pop().unwrap();
    let now = Instant::now();
    let mut leaving = Leaving::default();
    let deadline = leaving.begin(&editor, false, now).unwrap();
    assert_eq!(leaving.check_deadline(&editor, now), None, "not yet");
    assert_eq!(
        leaving.check_deadline(&editor, deadline),
        Some(AfterSave::StayOpen)
    );
    assert!(!leaving.is_under_way());

    // Both are stuck now, so the next exit goes without them.
    assert_eq!(leaving.begin(&editor, false, now), None);

    // The second ends, and is no longer stuck, while the first still is.
    let report = saver::run(second);
    saver::apply(&mut editor, report);
    assert_eq!(
        leaving.save_ended(&editor, false),
        None,
        "no exit under way"
    );
    assert_eq!(
        leaving.begin(&editor, false, now),
        None,
        "the first stays stuck"
    );

    // A new save of the same buffer is worth waiting for, once the old one has reported.
    send(&mut Dispatcher::new(), &mut editor, "y C-x C-s");
    let third = editor.take_save_jobs().pop().unwrap();
    assert!(leaving.begin(&editor, false, now).is_some(), "not stuck");
    assert_eq!(leaving.check_deadline(&editor, now), None);
    let report = saver::run(third);
    saver::apply(&mut editor, report);
    assert_eq!(
        leaving.save_ended(&editor, false),
        Some(AfterSave::Exit),
        "the first's stuck save does not hold it"
    );
    let report = saver::run(first);
    saver::apply(&mut editor, report);
    assert_eq!(leaving.save_ended(&editor, false), None);
}

#[test]
fn a_stuck_saves_entry_clears_once_it_reports() {
    let scratch = Scratch::new("dispatch-leave-clears");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    let stuck = editor.take_save_jobs().pop().unwrap();
    let now = Instant::now();
    let mut leaving = Leaving::default();
    let deadline = leaving.begin(&editor, false, now).unwrap();
    assert_eq!(
        leaving.check_deadline(&editor, deadline),
        Some(AfterSave::StayOpen)
    );
    assert_eq!(
        leaving.begin(&editor, false, now),
        None,
        "stuck: go at once"
    );
    let report = saver::run(stuck);
    saver::apply(&mut editor, report);
    assert_eq!(leaving.save_ended(&editor, false), None);

    // The entry has cleared, so another save of this buffer is waited for.
    send(&mut dispatcher, &mut editor, "y C-x C-s");
    editor.take_save_jobs();
    assert!(leaving.begin(&editor, false, now).is_some());
}

#[test]
fn the_sockets_exit_goes_at_the_limit() {
    let scratch = Scratch::new("dispatch-leave-socket-limit");
    let path = scratch.file("notes.txt", b"abc\n");
    let (mut dispatcher, mut editor) = (Dispatcher::new(), visiting(&path));
    send(&mut dispatcher, &mut editor, "x C-x C-s");
    let now = Instant::now();
    let mut leaving = Leaving::default();
    let deadline = leaving.begin(&editor, true, now).unwrap();
    assert_eq!(
        leaving.check_deadline(&editor, deadline),
        Some(AfterSave::Exit)
    );
}
