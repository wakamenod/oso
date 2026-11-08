//! The tests of writing under a new name, on a scratch directory with no window or thread.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::fs;

use oso_core::buffer::Replacement;
use oso_core::editor::{Editor, SaveRefused};

use super::super::tests::{Scratch, visit};
use super::super::{apply, run};
use crate::commands::kept_messages;

/// Puts `text` in what `editor` shows.
fn type_in(editor: &mut Editor, text: &str) {
    let id = editor.window().buffer();
    let edit = Replacement {
        range: 0..usize::MAX,
        text,
    };
    editor.apply_edits(id, &[edit]).unwrap();
}

/// Writes what `editor` shows to `path`, as the app would, and takes the report in.
fn write_to(editor: &mut Editor, path: &std::path::Path, over: bool) -> Result<(), SaveRefused> {
    editor.start_write(path.to_path_buf(), over)?;
    let job = editor.take_save_jobs().pop().unwrap();
    apply(editor, run(job));
    Ok(())
}

fn last_message(editor: &Editor) -> String {
    kept_messages(editor).pop().unwrap_or_default()
}

#[test]
fn a_new_file_is_written_and_the_buffer_visits_it_and_leaves_the_old_one() {
    let scratch = Scratch::new("elsewhere-new");
    let old = scratch.file("old.md", b"old\n");
    let mut editor = visit(&old);
    type_in(&mut editor, "new text");
    let new = scratch.path().join("new.md");
    write_to(&mut editor, &new, false).unwrap();
    assert_eq!(fs::read_to_string(&new).unwrap(), "new text\n");
    assert_eq!(fs::read_to_string(&old).unwrap(), "old\n", "untouched");
    let buffer = editor.current().1;
    assert_eq!(buffer.path(), Some(new.as_path()));
    assert_eq!(buffer.name(), "new.md");
    assert!(!buffer.is_modified());
    assert!(last_message(&editor).starts_with("Wrote "));
    // It saves where it now is.
    type_in(&mut editor, "again");
    editor.start_save().unwrap();
    let job = editor.take_save_jobs().pop().unwrap();
    apply(&mut editor, run(job));
    assert_eq!(fs::read_to_string(&new).unwrap(), "again\n");
    assert_eq!(fs::read_to_string(&old).unwrap(), "old\n");
}

#[test]
fn a_file_there_is_kept_until_the_person_agrees_and_then_written_over_once() {
    let scratch = Scratch::new("elsewhere-over");
    let old = scratch.file("old.md", b"old\n");
    let there = scratch.file("there.md", b"theirs\n");
    let mut editor = visit(&old);
    type_in(&mut editor, "mine");
    write_to(&mut editor, &there, false).unwrap();
    assert_eq!(fs::read_to_string(&there).unwrap(), "theirs\n", "kept");
    assert!(last_message(&editor).contains("C-x C-w asks to write over it"));
    assert_eq!(editor.write_over_asked(), Some(there.as_path()));
    assert_eq!(
        editor.current().1.path(),
        Some(old.as_path()),
        "nothing moved"
    );
    assert_eq!(
        write_to(&mut editor, &there, false),
        Err(SaveRefused::Exists)
    );
    write_to(&mut editor, &there, true).unwrap();
    assert_eq!(fs::read_to_string(&there).unwrap(), "mine\n");
    assert_eq!(editor.current().1.path(), Some(there.as_path()));
}

#[test]
fn a_file_changed_after_the_question_is_kept_and_asked_about_again() {
    let scratch = Scratch::new("elsewhere-changed");
    let old = scratch.file("old.md", b"old\n");
    let there = scratch.file("there.md", b"theirs\n");
    let mut editor = visit(&old);
    type_in(&mut editor, "mine");
    write_to(&mut editor, &there, false).unwrap();
    fs::write(&there, b"changed since, longer\n").unwrap();
    write_to(&mut editor, &there, true).unwrap();
    assert_eq!(
        fs::read_to_string(&there).unwrap(),
        "changed since, longer\n"
    );
    assert_eq!(editor.write_over_asked(), Some(there.as_path()));
}

#[test]
fn a_directory_is_refused_and_nothing_is_written() {
    let scratch = Scratch::new("elsewhere-directory");
    let old = scratch.file("old.md", b"old\n");
    let mut editor = visit(&old);
    type_in(&mut editor, "mine");
    let names = scratch.names();
    write_to(&mut editor, scratch.path(), false).unwrap();
    assert!(
        last_message(&editor).contains("it is a directory"),
        "{}",
        last_message(&editor)
    );
    assert_eq!(scratch.names(), names);
    assert!(editor.current().1.is_modified());
}

#[test]
fn a_missing_directory_is_not_made() {
    let scratch = Scratch::new("elsewhere-missing");
    let old = scratch.file("old.md", b"old\n");
    let mut editor = visit(&old);
    type_in(&mut editor, "mine");
    let new = scratch.path().join("missing").join("new.md");
    write_to(&mut editor, &new, false).unwrap();
    assert!(!scratch.path().join("missing").exists());
    assert!(last_message(&editor).starts_with("Not saved"));
    assert_eq!(editor.current().1.path(), Some(old.as_path()));
}

#[test]
fn the_new_path_is_judged_and_a_secret_stays_withheld() {
    let scratch = Scratch::new("elsewhere-secret");
    let old = scratch.file("old.md", b"old\n");
    let mut editor = visit(&old);
    assert!(!editor.current().1.is_secret());
    write_to(&mut editor, &scratch.path().join(".env"), false).unwrap();
    assert!(editor.current().1.is_secret(), "written to a secret name");
    write_to(&mut editor, &scratch.path().join("plain.md"), false).unwrap();
    assert!(
        editor.current().1.is_secret(),
        "a secret's text stays withheld"
    );
}

#[cfg(unix)]
#[test]
fn a_link_to_nothing_creates_its_target() {
    let scratch = Scratch::new("elsewhere-link");
    let old = scratch.file("old.md", b"old\n");
    let target = scratch.path().join("target.md");
    let link = scratch.path().join("link.md");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut editor = visit(&old);
    write_to(&mut editor, &link, false).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "old\n");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "a link stays a link"
    );
}
