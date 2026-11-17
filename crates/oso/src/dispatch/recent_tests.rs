//! Scenario tests of the recent files run by keys (ADR 0148): `C-x C-r` and its order, the
//! files `C-x b` lists after the buffers, and the person's command that puts a file first.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use oso_core::buffer::{Acted, Baseline, Buffer, FileStamp, Verdict};
use oso_core::complete::Candidate;
use oso_core::editor::{AskedVisit, Editor, Preview};
use oso_core::window::ViewSize;

use super::Dispatcher;
use super::motion_tests::send;

/// An absolute path on this system under a home directory.
fn home() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Users\me")
    } else {
        PathBuf::from("/home/me")
    }
}

/// A buffer visiting `path`, loaded, judged ordinary and on disk.
fn visiting(path: &Path) -> Buffer {
    let mut buffer = Buffer::visiting(path.to_path_buf());
    buffer.finish_empty();
    buffer.judge_secret(Some(&[path.to_path_buf()]));
    buffer.set_baseline(Baseline {
        location: path.to_path_buf(),
        file: Some(FileStamp {
            modified: SystemTime::UNIX_EPOCH,
            len: 0,
            mode: 0,
            identity: None,
        }),
    });
    buffer
}

/// An editor on `*scratch*` that remembers `todo.md` and then `main.rs`.
fn remembering() -> (Dispatcher, Editor) {
    let mut editor = Editor::default();
    editor.set_view_size(ViewSize::new(24, 80));
    editor.set_home(Some(home()));
    editor.remember_recent_files([home().join("todo.md"), home().join("main.rs")]);
    (Dispatcher::new(), editor)
}

fn rows(editor: &Editor) -> Vec<(String, String)> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    candidates
        .shown(18)
        .map(|(_, candidate)| {
            (
                candidate.text().to_owned(),
                candidate.annotation().to_owned(),
            )
        })
        .collect()
}

fn selected(editor: &Editor) -> Option<&str> {
    let candidates = editor.prompt()?.candidates()?;
    candidates.selected_candidate().map(Candidate::text)
}

#[test]
fn c_x_c_r_opens_the_file_chosen_as_a_person_and_notes_the_choice() {
    let (mut d, mut editor) = remembering();
    send(&mut d, &mut editor, "C-x C-r");
    assert_eq!(editor.prompt().unwrap().label(), "Open recent file:");
    assert_eq!(selected(&editor), Some("~/todo.md"));
    send(&mut d, &mut editor, "C-n RET");
    assert!(editor.prompt().is_none());
    // Marked as chosen from the list, so that a load that finds nothing there drops it.
    let asked = AskedVisit {
        path: home().join("main.rs"),
        limit: oso_core::load::UNASKED,
        by: Acted::Person,
        from_recent: true,
    };
    assert_eq!(editor.take_asked_visits(), [asked]);
}

#[test]
fn a_large_recent_file_asks_first_and_its_y_visits_it_as_any() {
    let (mut d, mut editor) = remembering();
    editor.remember_large(home().join("todo.md"), 3 << 30);
    send(&mut d, &mut editor, "C-x C-r RET");
    assert!(editor.take_asked_visits().is_empty(), "asked first");
    send(&mut d, &mut editor, "y");
    let asked = editor.take_asked_visits();
    assert_eq!(asked.len(), 1);
    assert!(!asked[0].from_recent, "{asked:?}");
}

#[test]
fn a_file_c_x_f_visits_is_not_marked_as_from_the_list() {
    let (mut d, mut editor) = remembering();
    send(&mut d, &mut editor, "C-x C-r RET");
    let _ = editor.take_asked_visits();
    editor.request_visit(home().join("main.rs"), oso_core::load::UNASKED);
    assert!(
        editor
            .take_asked_visits()
            .iter()
            .all(|visit| !visit.from_recent)
    );
}

#[test]
fn c_x_c_r_takes_only_a_listed_file_and_refuses_inside_a_prompt() {
    let (mut d, mut editor) = remembering();
    send(&mut d, &mut editor, "C-x C-r z z z RET");
    assert!(editor.prompt().is_some(), "nothing matches");
    assert!(editor.take_visits().is_empty());
    send(&mut d, &mut editor, "C-g C-x b C-x C-r");
    assert_eq!(editor.prompt().unwrap().label(), "Switch to buffer:");
}

#[test]
fn the_current_buffers_file_comes_last_and_an_open_one_says_so() {
    let (mut d, mut editor) = remembering();
    let todo = editor.add_buffer(visiting(&home().join("todo.md")));
    editor.show(todo);
    send(&mut d, &mut editor, "C-x C-r");
    assert_eq!(
        rows(&editor),
        [
            ("~/main.rs".to_owned(), String::new()),
            ("~/todo.md".to_owned(), "open".to_owned())
        ]
    );
    // Its buffer is shown again rather than the file loaded.
    send(&mut d, &mut editor, "C-n RET");
    assert!(editor.take_visits().is_empty());
    assert_eq!(editor.current().1.name(), "todo.md");
}

#[test]
fn c_x_b_lists_the_files_no_buffer_visits_after_the_buffers_and_opens_one() {
    let (mut d, mut editor) = remembering();
    let todo = editor.add_buffer(visiting(&home().join("todo.md")));
    editor.show(todo);
    send(&mut d, &mut editor, "C-x b");
    assert_eq!(
        rows(&editor)
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>(),
        ["*scratch*", "*Messages*", "todo.md", "~/main.rs"]
    );
    assert_eq!(rows(&editor)[3].1, "file");
    send(&mut d, &mut editor, "C-n C-n C-n");
    assert_eq!(editor.preview(), Some(Preview::NotOpen));
    send(&mut d, &mut editor, "RET");
    let visits = editor.take_visits();
    assert_eq!(visits, [(home().join("main.rs"), oso_core::load::UNASKED)]);
    // `C-x k` lists no files.
    send(&mut d, &mut editor, "C-x k");
    assert!(rows(&editor).iter().all(|(text, _)| !text.starts_with('~')));
}

#[test]
fn a_persons_command_puts_the_file_shown_first() {
    let (mut d, mut editor) = remembering();
    let main = editor.add_buffer(visiting(&home().join("main.rs")));
    editor.show(main);
    assert_eq!(
        editor.recent_files()[0],
        home().join("todo.md"),
        "not before a key"
    );
    send(&mut d, &mut editor, "C-f");
    assert_eq!(editor.recent_files()[0], home().join("main.rs"));
    assert_eq!(editor.take_recent_changes().shown, [home().join("main.rs")]);
}

#[test]
fn a_file_that_may_hold_secrets_is_never_kept() {
    let (mut d, mut editor) = remembering();
    let path = home().join(".env");
    let mut secret = visiting(&path);
    secret.judge_secret(Some(&[path]));
    assert_eq!(secret.verdict(), Verdict::SecretName);
    let secret = editor.add_buffer(secret);
    editor.show(secret);
    send(&mut d, &mut editor, "C-f");
    assert_eq!(editor.recent_files().len(), 2);
    assert!(editor.take_recent_changes().is_empty());
}

#[test]
fn the_agent_channel_neither_opens_a_recent_file_nor_lists_them() {
    use oso_core::command::Reach;
    let (mut d, mut editor) = remembering();
    let reach = Reach::Change;
    let given = super::Given::Text("~/main.rs");
    assert!(
        d.run_for_agent(&mut editor, "switch-to-buffer", given, reach)
            .is_err()
    );
    let nothing = super::Given::Nothing;
    assert!(
        d.run_for_agent(&mut editor, "recentf-open", nothing, reach)
            .is_err()
    );
    assert!(editor.take_visits().is_empty());
    assert!(editor.prompt().is_none());
}
