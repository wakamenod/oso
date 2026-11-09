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
fn a_directory_that_holds_the_name_already_asks_and_writes_nothing() {
    let scratch = Scratch::new("elsewhere-directory");
    let old = scratch.file("old.md", b"old\n");
    let mut editor = visit(&old);
    type_in(&mut editor, "mine");
    let names = scratch.names();
    write_to(&mut editor, scratch.path(), false).unwrap();
    assert!(
        last_message(&editor).contains("C-x C-w asks to write over it"),
        "{}",
        last_message(&editor)
    );
    assert_eq!(scratch.names(), names);
    assert_eq!(fs::read_to_string(&old).unwrap(), "old\n");
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

#[cfg(unix)]
#[test]
fn another_buffers_file_is_refused_through_a_link_or_a_second_name() {
    let scratch = Scratch::new("elsewhere-open");
    let old = scratch.file("old.md", b"old\n");
    let open = scratch.file("open.md", b"open\n");
    let link = scratch.path().join("link.md");
    std::os::unix::fs::symlink(&open, &link).unwrap();
    let second = scratch.path().join("second.md");
    fs::hard_link(&open, &second).unwrap();
    let mut editor = visit(&old);
    editor.add_buffer(super::super::tests::visited(&open));
    for path in [&link, &second] {
        write_to(&mut editor, path, false).unwrap();
        assert!(
            last_message(&editor).contains("open in another buffer"),
            "{}",
            last_message(&editor)
        );
    }
    assert_eq!(fs::read_to_string(&open).unwrap(), "open\n");
    assert_eq!(editor.current().1.path(), Some(old.as_path()));
}

#[cfg(unix)]
#[test]
fn a_path_that_leads_elsewhere_after_the_question_writes_nothing() {
    let scratch = Scratch::new("elsewhere-moved");
    let old = scratch.file("old.md", b"old\n");
    let there = scratch.file("there.md", b"theirs\n");
    let away = scratch.file("away.md", b"theirs\n");
    let mut editor = visit(&old);
    type_in(&mut editor, "mine");
    write_to(&mut editor, &there, false).unwrap();
    // The same time and size, through a link to another file.
    fs::remove_file(&there).unwrap();
    std::os::unix::fs::symlink(&away, &there).unwrap();
    write_to(&mut editor, &there, true).unwrap();
    assert_eq!(fs::read_to_string(&away).unwrap(), "theirs\n");
    assert!(last_message(&editor).starts_with("Not saved"));
}

#[test]
fn a_file_that_comes_before_the_rename_is_not_replaced() {
    let scratch = Scratch::new("elsewhere-race");
    let (temporary, file) = super::super::Temporary::create(scratch.path(), 0o600).unwrap();
    drop(file);
    let target = scratch.file("target.md", b"came meanwhile\n");
    let put = temporary.put_where_nothing_is(&target);
    assert_eq!(put.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(&target).unwrap(), "came meanwhile\n");
    assert_eq!(scratch.names(), ["target.md"], "the temporary file went");
}

#[test]
fn a_new_file_is_put_in_place_under_its_name_only() {
    let scratch = Scratch::new("elsewhere-put");
    let (temporary, file) = super::super::Temporary::create(scratch.path(), 0o600).unwrap();
    drop(file);
    let target = scratch.path().join("new.md");
    temporary.put_where_nothing_is(&target).unwrap();
    assert_eq!(scratch.names(), ["new.md"]);
}

#[test]
fn a_directory_takes_the_buffers_file_name() {
    let scratch = Scratch::new("elsewhere-into");
    let old = scratch.file("old.md", b"old\n");
    let into = scratch.path().join("into");
    fs::create_dir(&into).unwrap();
    let mut editor = visit(&old);
    type_in(&mut editor, "mine");
    write_to(&mut editor, &into, false).unwrap();
    assert_eq!(fs::read_to_string(into.join("old.md")).unwrap(), "mine\n");
    assert_eq!(
        editor.current().1.path(),
        Some(into.join("old.md").as_path())
    );
    let named = into.join("old.md").display().to_string();
    assert!(
        last_message(&editor).ends_with(&named),
        "{}",
        last_message(&editor)
    );
}
