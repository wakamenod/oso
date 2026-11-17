//! Whether a buffer is highlighted whole (ADR 0122, step 9): the glue's mark of each buffer's
//! done report, gone with an edit and with a new start at the same revision.
//!
//! Must not depend on: threads or the syntax worker; reports are given by hand.

use oso_core::syntax::Highlights;
use oso_syntax::{Outcome, Report};

use super::Syntax;
use super::tests::{edit, editor_with, visited};

#[test]
fn a_done_report_says_the_buffer_is_whole_until_its_next_edit() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
    let job = syntax.orders(&editor).jobs.remove(0);
    let report = |stamp, revision| Report {
        buffer: id,
        epoch: job.epoch,
        stamp,
        revision,
        outcome: Outcome::Highlights(Highlights::new(job.text.len())),
        done: true,
    };
    assert!(syntax.take(&mut editor, report(1, job.revision)).taken);
    let revision = editor.buffer(id).unwrap().revision();
    assert!(syntax.is_whole(id, revision));
    edit(&mut editor, id, 0, "x");
    let now = editor.buffer(id).unwrap().revision();
    assert!(!syntax.is_whole(id, now), "edited since");
    // A done report of the text before the edit says nothing of the text now.
    let _ = syntax.orders(&editor);
    syntax.take(&mut editor, report(2, job.revision));
    assert!(!syntax.is_whole(id, now));
}

#[test]
fn a_new_start_at_the_same_revision_is_not_whole_until_its_own_done_report() {
    let (mut editor, id) = editor_with(visited("/p/main.rs", "fn a() {}\n"));
    let mut syntax = Syntax::default();
    let job = syntax.orders(&editor).jobs.remove(0);
    let report = |epoch, stamp, done| Report {
        buffer: id,
        epoch,
        stamp,
        revision: job.revision,
        outcome: Outcome::Highlights(Highlights::new(job.text.len())),
        done,
    };
    assert!(syntax.take(&mut editor, report(job.epoch, 1, true)).taken);
    let revision = editor.buffer(id).unwrap().revision();
    assert!(syntax.is_whole(id, revision));
    // Highlighting off and on: dropped, then started anew at the same revision.
    editor.set_font_lock(false);
    let _ = syntax.drops(&editor);
    assert!(!syntax.is_whole(id, revision), "dropped");
    editor.set_font_lock(true);
    let again = syntax.orders(&editor).jobs.remove(0);
    assert_eq!(again.revision, job.revision);
    assert_ne!(again.epoch, job.epoch);
    assert!(
        syntax
            .take(&mut editor, report(again.epoch, 1, false))
            .taken
    );
    assert!(!syntax.is_whole(id, revision), "partly, in the new start");
    assert!(syntax.take(&mut editor, report(again.epoch, 2, true)).taken);
    assert!(syntax.is_whole(id, revision));
}
