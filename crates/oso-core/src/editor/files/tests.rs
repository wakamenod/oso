//! Tests of the prompt that reads a path, with listings made up.
//!
//! Must not depend on: threads or the file system.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::super::edit_tests::edit;
use crate::complete::path::{Name, NameKind};
use crate::editor::Editor;
use crate::minibuffer::Outcome;

fn names(entries: &[(&str, NameKind)]) -> Option<Arc<[Name]>> {
    Some(
        entries
            .iter()
            .map(|(name, kind)| Name {
                name: (*name).to_owned(),
                kind: *kind,
            })
            .collect(),
    )
}

/// An editor with a prompt for a path open at `~/`, home being `/home/me`, its listing given.
fn prompt() -> Editor {
    let mut editor = Editor::default();
    editor.set_home(Some(PathBuf::from("/home/me")));
    editor
        .open_file_prompt("Find file:", "file", "~/", PathBuf::from("/work"))
        .unwrap();
    assert_eq!(editor.take_file_requests(), [PathBuf::from("/home/me")]);
    assert!(editor.files_pending());
    use NameKind::{Directory as D, File as F};
    editor.file_listed(
        PathBuf::from("/home/me"),
        names(&[("notes", D), ("todo.md", F), (".config", D)]),
        false,
    );
    editor
}

fn rows(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    candidates
        .shown(18)
        .map(|(_, candidate)| candidate.text().to_owned())
        .collect()
}

fn answer(editor: &Editor) -> String {
    let id = editor.prompt().unwrap().buffer();
    editor.buffer(id).unwrap().text().to_string()
}

fn typing(editor: &mut Editor, text: &str) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, text)]).unwrap();
    editor.match_completion();
}

#[test]
fn a_listing_fills_the_list_without_hidden_names() {
    let editor = prompt();
    assert!(!editor.files_pending());
    assert_eq!(rows(&editor), ["notes/", "todo.md"]);
    assert_eq!(editor.file_directory(), Some(Path::new("/home/me")));
}

#[test]
fn the_last_segment_is_matched_and_a_new_directory_is_asked_for() {
    let mut editor = prompt();
    typing(&mut editor, "do");
    assert_eq!(rows(&editor), ["todo.md"], "the matcher over names only");
    typing(&mut editor, "/");
    assert_eq!(
        editor.take_file_requests(),
        Vec::<PathBuf>::new(),
        "no todo.md/ to list"
    );
}

#[test]
fn tab_puts_the_whole_path_in_and_ret_on_a_directory_goes_into_it() {
    let mut editor = prompt();
    editor.insert_completion().unwrap();
    assert_eq!(answer(&editor), "~/notes/");
    assert_eq!(
        editor.take_file_requests(),
        [PathBuf::from("/home/me/notes")]
    );
    editor.file_listed(
        PathBuf::from("/home/me/notes"),
        names(&[("plan.md", NameKind::File)]),
        false,
    );
    assert_eq!(rows(&editor), ["plan.md"]);
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(Path::new(&text), Path::new("/home/me/notes/plan.md"));
}

#[test]
fn ret_on_a_directory_goes_into_it_and_stays_open() {
    let mut editor = prompt();
    editor.submit_prompt().unwrap();
    assert!(editor.prompt().is_some());
    assert_eq!(answer(&editor), "~/notes/");
}

#[test]
fn a_path_that_matches_nothing_is_given_as_typed() {
    let mut editor = prompt();
    typing(&mut editor, "new.md");
    assert!(rows(&editor).is_empty());
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(Path::new(&text), Path::new("/home/me/new.md"));
}

#[test]
fn m_ret_gives_what_is_typed_while_a_name_matches() {
    let mut editor = prompt();
    typing(&mut editor, "to");
    editor.submit_typed().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(Path::new(&text), Path::new("/home/me/to"));
}

#[test]
fn a_directory_that_cannot_be_read_says_so_and_lists_nothing() {
    let mut editor = Editor::default();
    editor
        .open_file_prompt("Find file:", "file", "/locked/", PathBuf::from("/work"))
        .unwrap();
    // The directory it starts in is known, so the one above it needs no listing.
    assert_eq!(editor.take_file_requests(), [PathBuf::from("/locked")]);
    editor.file_listed(PathBuf::from("/locked"), None, false);
    assert!(rows(&editor).is_empty());
    assert_eq!(editor.prompt().unwrap().message(), Some("Cannot list it"));
}

#[test]
fn a_listing_after_the_prompt_closed_changes_nothing() {
    let mut editor = prompt();
    editor.quit();
    editor.file_listed(PathBuf::from("/home/me/notes"), names(&[]), false);
    assert!(editor.prompt().is_none());
    assert_eq!(editor.take_file_requests(), Vec::<PathBuf>::new());
}

/// Answers every listing the prompt asks for from `tree`, a listing cut short where `cut`.
fn serve(editor: &mut Editor, tree: &[(&str, &[(&str, NameKind)])], cut: bool) {
    for _ in 0..8 {
        let asked = editor.take_file_requests();
        if asked.is_empty() {
            return;
        }
        for directory in asked {
            let listed = tree
                .iter()
                .find(|(path, _)| Path::new(path) == directory)
                .and_then(|(_, entries)| names(entries));
            editor.file_listed(directory, listed, cut);
        }
    }
}

#[test]
fn only_the_name_is_matched_beside_the_directories_that_tell_rows_apart() {
    use NameKind::{Directory as D, File as F};
    let mut editor = Editor::default();
    editor
        .open_file_prompt("Find file:", "file", "/h/", PathBuf::from("/"))
        .unwrap();
    typing(&mut editor, "D/");
    let tree: &[(&str, &[(&str, NameKind)])] = &[
        ("/", &[("h", D)]),
        ("/h", &[("Desktop", D), ("Documents", D)]),
        ("/h/Desktop", &[("a.txt", F)]),
        ("/h/Documents", &[("b.md", F)]),
    ];
    serve(&mut editor, tree, false);
    assert_eq!(rows(&editor), ["b.md", "a.txt"], "shorter first");
    typing(&mut editor, "t");
    assert_eq!(
        rows(&editor),
        ["a.txt"],
        "not every row for the t of Desktop"
    );
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(candidates.get(0).unwrap().annotation(), "Desktop/");
}

#[test]
fn a_listing_no_plan_waits_for_keeps_the_selection_where_it_is() {
    let mut editor = prompt();
    editor.move_completion(1).unwrap();
    editor.file_listed(PathBuf::from("/elsewhere"), names(&[]), false);
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(
        candidates.selected_candidate().map(|c| c.text()),
        Some("todo.md")
    );
}

#[test]
fn a_question_over_the_prompt_keeps_it_open_for_its_listings() {
    let mut editor = prompt();
    editor.ask("Go on?", "keyboard-quit").unwrap();
    assert!(editor.file_prompt_open());
    assert!(!editor.files_pending());
}

#[test]
fn a_listing_cut_short_shows_in_the_count() {
    let mut editor = Editor::default();
    editor
        .open_file_prompt("Find file:", "file", "/big/", PathBuf::from("/"))
        .unwrap();
    let tree: &[(&str, &[(&str, NameKind)])] = &[
        ("/", &[("big", NameKind::Directory)]),
        ("/big", &[("a", NameKind::File)]),
    ];
    serve(&mut editor, tree, true);
    assert!(editor.files_cut());
}

fn looked(annotation: &str, bytes: u64, directory: bool) -> crate::editor::Looked {
    crate::editor::Looked {
        annotation: annotation.to_owned(),
        bytes,
        directory,
    }
}

#[test]
fn the_rows_shown_are_looked_at_once_and_their_annotations_fill_in() {
    let mut editor = prompt();
    let asked = editor.take_file_looks();
    assert_eq!(
        asked,
        [(
            PathBuf::from("/home/me"),
            vec!["notes".to_owned(), "todo.md".to_owned()]
        )]
    );
    assert!(editor.take_file_looks().is_empty(), "once");
    editor.move_completion(1).unwrap();
    let home = Path::new("/home/me");
    editor.file_looked(
        home,
        vec![("todo.md".to_owned(), looked("-rw-r--r--  12", 12, false))],
    );
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    let todo = candidates.get(1).unwrap();
    assert_eq!(todo.annotation(), "-rw-r--r--  12");
    assert_eq!(
        candidates.selected_candidate().map(|c| c.text()),
        Some("todo.md"),
        "the selection stays"
    );
}

#[test]
fn a_link_looked_at_as_a_directory_is_gone_into() {
    let mut editor = Editor::default();
    editor
        .open_file_prompt("Find file:", "file", "/r/", PathBuf::from("/"))
        .unwrap();
    editor.take_file_requests();
    editor.file_listed(
        PathBuf::from("/r"),
        names(&[("to-dir", NameKind::Link)]),
        false,
    );
    editor.file_looked(
        Path::new("/r"),
        vec![("to-dir".to_owned(), looked("l", 0, true))],
    );
    assert_eq!(
        rows(&editor),
        ["to-dir/"],
        "a link to a directory reads as one"
    );
    editor.submit_prompt().unwrap();
    assert!(editor.prompt().is_some(), "went into it");
    assert_eq!(answer(&editor), "/r/to-dir/");
}

#[test]
fn a_file_a_look_found_large_is_remembered_so_opening_it_asks() {
    let mut editor = prompt();
    editor.move_completion(1).unwrap();
    let home = Path::new("/home/me");
    editor.file_looked(
        home,
        vec![("todo.md".to_owned(), looked("", 3 << 30, false))],
    );
    editor.submit_prompt().unwrap();
    assert_eq!(
        editor.large_file(Path::new("/home/me/todo.md")),
        Some(3 << 30)
    );
}

#[test]
fn a_buffer_that_loaded_the_file_another_has_is_found_by_its_identity() {
    use crate::buffer::{Baseline, Buffer, FileStamp};
    let stamp = FileStamp {
        modified: std::time::SystemTime::UNIX_EPOCH,
        len: 1,
        mode: 0o644,
        identity: Some((1, 42)),
    };
    let visiting = |path: &str| {
        let mut buffer = Buffer::visiting(PathBuf::from(path));
        buffer.finish_empty();
        buffer.set_baseline(Baseline {
            location: PathBuf::from(path),
            file: Some(stamp),
        });
        buffer
    };
    let mut editor = Editor::default();
    let first = editor.add_buffer(visiting("/a/notes.md"));
    let second = editor.add_buffer(visiting("/b/link.md"));
    assert_eq!(editor.visiting_same_file(second), Some(first));
    let third = editor.add_buffer(Buffer::from_text("other", ""));
    assert_eq!(editor.visiting_same_file(third), None, "no file");
    // An open the agent channel has not shown yet is left to its checks: it neither gives way
    // to the person's buffer nor is found as the one open, by its identity or its path.
    let mut agents = visiting("/c/again.md");
    agents.mark_opened_by_agent();
    agents.set_agent_pending(true);
    let agents = editor.add_buffer(agents);
    assert_eq!(editor.visiting_same_file(agents), None, "nothing dropped");
    editor.kill_buffer(first);
    assert_eq!(editor.visiting_same_file(second), None, "not found");
    editor.request_visit(PathBuf::from("/c/again.md"), 0);
    assert_eq!(editor.take_visits().len(), 1, "a load of its own");
    // Once the channel has shown it, it is the buffer of its file like any other.
    editor.buffer_mut(agents).unwrap().set_agent_pending(false);
    assert_eq!(editor.visiting_same_file(second), Some(agents));
    let count = editor.buffers().count();
    editor.request_visit(PathBuf::from("/c/again.md"), 0);
    assert!(editor.take_visits().is_empty(), "no second buffer");
    assert_eq!(editor.buffers().count(), count);
    assert_eq!(editor.window().buffer(), agents, "shown");
}

#[test]
fn a_directory_prompt_lists_no_file_and_gives_a_directory_typed_with_a_slash() {
    let mut editor = Editor::default();
    editor.set_home(Some(PathBuf::from("/home/me")));
    editor
        .open_directory_prompt("Search in directory:", "dir", "~/", PathBuf::from("/work"))
        .unwrap();
    editor.take_file_requests();
    use NameKind::{Directory as D, File as F, Link as L, Other as O};
    editor.file_listed(
        PathBuf::from("/home/me"),
        names(&[("notes", D), ("todo.md", F), ("ln", L), ("pipe", O)]),
        false,
    );
    // Only directories and links, which may lead to one, as `read-directory-name` lists.
    assert_eq!(rows(&editor), ["ln", "notes/"]);
    // What is typed with `/` at its end is the directory, whatever row is selected.
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(0..end, "~/notes/")]).unwrap();
    editor.match_completion();
    editor.take_file_requests();
    editor.file_listed(
        PathBuf::from("/home/me/notes"),
        names(&[("a.md", F)]),
        false,
    );
    assert!(rows(&editor).is_empty(), "a directory of files lists none");
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    // A path of the system's, `/` at its end saying it is a directory.
    assert_eq!(PathBuf::from(&text), Path::new("/home/me").join("notes"));
    assert!(text.ends_with('/'));
}

/// A prompt for a directory at `~/`, home being `/home/me`, its listing given.
fn directory_prompt() -> Editor {
    let mut editor = Editor::default();
    editor.set_home(Some(PathBuf::from("/home/me")));
    editor
        .open_directory_prompt("Search in directory:", "dir", "~/", PathBuf::from("/work"))
        .unwrap();
    editor.take_file_requests();
    use NameKind::{Directory as D, File as F, Link as L};
    editor.file_listed(
        PathBuf::from("/home/me"),
        names(&[("notes", D), ("todo.md", F), ("ln", L)]),
        false,
    );
    editor
}

fn message(editor: &Editor) -> Option<&str> {
    editor.prompt().and_then(|prompt| prompt.message())
}

#[test]
fn a_directory_prompt_goes_into_a_directorys_row() {
    let mut editor = directory_prompt();
    typing(&mut editor, "not");
    editor.submit_prompt().unwrap();
    assert!(editor.prompt().is_some(), "still open");
    assert_eq!(answer(&editor), "~/notes/");
}

#[test]
fn a_directory_prompt_refuses_a_file_typed_whole_and_a_name_its_listing_lacks() {
    for typed in [true, false] {
        let mut editor = directory_prompt();
        typing(&mut editor, "todo.md");
        let submitted = if typed {
            editor.submit_typed()
        } else {
            // No row matches, so what is typed is taken.
            editor.submit_prompt()
        };
        assert!(submitted.is_err(), "typed: {typed}");
        assert_eq!(message(&editor), Some("Not a directory: ~/todo.md"));
    }
    let mut editor = directory_prompt();
    typing(&mut editor, "elsewhere");
    assert!(editor.submit_typed().is_err());
    assert_eq!(message(&editor), Some("No such directory: ~/elsewhere"));
    assert!(editor.prompt().is_some(), "it stays open");
    // A directory typed whole is given.
    let mut editor = directory_prompt();
    typing(&mut editor, "notes");
    editor.submit_typed().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(PathBuf::from(&text), Path::new("/home/me").join("notes"));
}

#[test]
fn a_directory_prompt_waits_for_the_listing_that_decides_its_answer() {
    let mut editor = Editor::default();
    editor.set_home(Some(PathBuf::from("/home/me")));
    editor
        .open_directory_prompt("Search in directory:", "dir", "~/", PathBuf::from("/work"))
        .unwrap();
    editor.take_file_requests();
    // Typed before `~/` is listed: `RET` and `M-RET` wait, as the host holds them.
    typing(&mut editor, "todo.md");
    assert!(editor.directory_answer_waits());
    editor.file_listed(
        PathBuf::from("/home/me"),
        names(&[("todo.md", NameKind::File)]),
        false,
    );
    assert!(!editor.directory_answer_waits());
    assert!(editor.submit_typed().is_err());
    assert_eq!(message(&editor), Some("Not a directory: ~/todo.md"));
    // A prompt for a file never waits so.
    let mut editor = Editor::default();
    editor
        .open_file_prompt("Find file:", "file", "/r/", PathBuf::from("/"))
        .unwrap();
    editor.take_file_requests();
    assert!(!editor.directory_answer_waits());
}

#[test]
fn a_directory_prompt_says_a_directory_it_cannot_read() {
    let mut editor = Editor::default();
    editor.set_home(Some(PathBuf::from("/home/me")));
    editor
        .open_directory_prompt("Search in directory:", "dir", "~/", PathBuf::from("/work"))
        .unwrap();
    editor.take_file_requests();
    editor.file_listed(PathBuf::from("/home/me"), None, false);
    typing(&mut editor, "x");
    assert!(editor.submit_typed().is_err());
    assert_eq!(message(&editor), Some("Cannot read directory: ~/"));
    let mut editor = directory_prompt();
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(0..end, "~/notes/")]).unwrap();
    editor.match_completion();
    editor.take_file_requests();
    editor.file_listed(PathBuf::from("/home/me/notes"), None, false);
    assert!(editor.submit_prompt().is_err());
    assert_eq!(message(&editor), Some("Cannot read directory: ~/notes/"));
}

#[test]
fn a_directory_prompt_takes_a_link_a_look_found_leads_to_a_directory() {
    // The link selected waits for its look, before it is asked for as after.
    let mut editor = directory_prompt();
    typing(&mut editor, "ln");
    assert!(editor.directory_answer_waits(), "not asked yet");
    editor.take_file_looks();
    assert!(editor.directory_answer_waits());
    editor.file_looked(
        Path::new("/home/me"),
        vec![("ln".to_owned(), looked("l", 0, false))],
    );
    assert!(!editor.directory_answer_waits());
    assert!(editor.submit_prompt().is_err());
    assert_eq!(message(&editor), Some("Not a directory: ~/ln"));
    assert!(editor.submit_typed().is_err(), "typed whole too");
    // One a look found leads to a directory is gone into, or given typed whole.
    let mut editor = directory_prompt();
    typing(&mut editor, "ln");
    editor.take_file_looks();
    editor.file_looked(
        Path::new("/home/me"),
        vec![("ln".to_owned(), looked("l", 0, true))],
    );
    editor.submit_typed().unwrap();
    assert!(editor.prompt().is_none());
}
