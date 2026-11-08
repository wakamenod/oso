//! Scenario tests of `C-x C-f` and `C-x C-w` run by keys (ADR 0087), with listings given as the
//! lister would: the start, going into a directory, a new file, a file open already, a large
//! file's question, and writing over a file after its question.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use oso_core::buffer::{Buffer, FileStamp};
use oso_core::complete::path::{Name, NameKind};
use oso_core::editor::Editor;
use oso_core::window::ViewSize;

use super::Dispatcher;
use super::motion_tests::send;

/// An editor on `/notes/a.md`, with `/notes` listing a directory and two files.
fn on_notes() -> (Dispatcher, Editor) {
    let mut buffer = Buffer::visiting(PathBuf::from("/notes/a.md"));
    buffer.finish_empty();
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(24, 80));
    editor.set_home(Some(PathBuf::from("/home/me")));
    (Dispatcher::new(), editor)
}

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

/// Answers every listing the prompt asks for from `tree`.
fn serve(editor: &mut Editor, tree: &[(&str, &[(&str, NameKind)])]) {
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
            editor.file_listed(directory, listed);
        }
    }
}

const TREE: &[(&str, &[(&str, NameKind)])] = &[
    ("/", &[("notes", NameKind::Directory)]),
    (
        "/notes",
        &[
            ("plans", NameKind::Directory),
            ("a.md", NameKind::File),
            ("b.md", NameKind::File),
        ],
    ),
    ("/notes/plans", &[("q3.md", NameKind::File)]),
];

fn rows(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    candidates
        .shown(18)
        .map(|(_, candidate)| candidate.text().to_owned())
        .collect()
}

#[test]
fn c_x_c_f_starts_at_the_buffers_directory_and_opens_the_file_chosen() {
    let (mut d, mut editor) = on_notes();
    send(&mut d, &mut editor, "C-x C-f");
    let prompt = editor.prompt().unwrap();
    assert_eq!(prompt.label(), "Find file:");
    serve(&mut editor, TREE);
    assert_eq!(rows(&editor), ["a.md", "b.md", "plans/"]);
    send(&mut d, &mut editor, "b RET");
    assert!(editor.prompt().is_none());
    assert_eq!(
        editor.take_visits(),
        [(PathBuf::from("/notes/b.md"), crate::loader::UNASKED)]
    );
}

#[test]
fn ret_on_a_directory_goes_into_it() {
    let (mut d, mut editor) = on_notes();
    send(&mut d, &mut editor, "C-x C-f");
    serve(&mut editor, TREE);
    send(&mut d, &mut editor, "p l RET");
    serve(&mut editor, TREE);
    assert_eq!(rows(&editor), ["q3.md"]);
    send(&mut d, &mut editor, "RET");
    assert_eq!(
        editor.take_visits()[0].0,
        PathBuf::from("/notes/plans/q3.md")
    );
}

#[test]
fn a_name_that_matches_nothing_opens_as_a_new_file() {
    let (mut d, mut editor) = on_notes();
    send(&mut d, &mut editor, "C-x C-f");
    serve(&mut editor, TREE);
    send(&mut d, &mut editor, "n e w RET");
    assert_eq!(editor.take_visits()[0].0, PathBuf::from("/notes/new"));
}

#[test]
fn a_file_open_already_shows_its_buffer() {
    let (mut d, mut editor) = on_notes();
    let other = editor.add_buffer(Buffer::from_text("other", "x"));
    editor.show(other);
    // A buffer with no file starts the path where oso started, `/` here.
    send(&mut d, &mut editor, "C-x C-f");
    serve(&mut editor, TREE);
    send(&mut d, &mut editor, "n o t e s /");
    serve(&mut editor, TREE);
    send(&mut d, &mut editor, "a . m d RET");
    assert!(editor.take_visits().is_empty());
    assert_eq!(editor.current().1.path(), Some(Path::new("/notes/a.md")));
}

#[test]
fn a_large_file_a_load_stopped_at_asks_and_y_opens_it() {
    let (mut d, mut editor) = on_notes();
    editor.remember_large(PathBuf::from("/notes/b.md"), 3 << 30);
    send(&mut d, &mut editor, "C-x C-f");
    serve(&mut editor, TREE);
    send(&mut d, &mut editor, "b RET");
    let question = editor.prompt().unwrap().label().to_owned();
    assert!(question.starts_with("b.md is 3.0 GiB"), "{question}");
    assert!(editor.take_visits().is_empty(), "nothing yet");
    send(&mut d, &mut editor, "y");
    assert_eq!(
        editor.take_visits(),
        [(PathBuf::from("/notes/b.md"), 3 << 30)]
    );
}

#[test]
fn c_x_c_w_writes_elsewhere_and_asks_over_a_file_found_there() {
    let (mut d, mut editor) = on_notes();
    send(&mut d, &mut editor, "x C-x C-w");
    assert_eq!(editor.prompt().unwrap().label(), "Write file:");
    serve(&mut editor, TREE);
    send(&mut d, &mut editor, "c . m d RET");
    let job = editor.take_save_jobs().pop().unwrap();
    assert_eq!(job.path, PathBuf::from("/notes/c.md"));
    assert!(job.elsewhere.is_some());
    // The save thread found a file there: the next C-x C-w of it asks.
    let id = editor.window().buffer();
    editor.save_failed(id);
    let found = FileStamp {
        modified: SystemTime::UNIX_EPOCH,
        len: 3,
        mode: 0o644,
        identity: None,
    };
    editor.may_write_over(
        id,
        PathBuf::from("/notes/c.md"),
        found,
        PathBuf::from("/notes/c.md"),
    );
    send(&mut d, &mut editor, "C-x C-w");
    serve(&mut editor, TREE);
    send(&mut d, &mut editor, "c . m d M-RET");
    let question = editor.prompt().unwrap().label().to_owned();
    assert!(question.starts_with("c.md exists"), "{question}");
    send(&mut d, &mut editor, "y");
    let job = editor.take_save_jobs().pop().unwrap();
    assert!(job.elsewhere.and_then(|elsewhere| elsewhere.over).is_some());
}

#[test]
fn c_x_c_f_refuses_inside_a_prompt() {
    let (mut d, mut editor) = on_notes();
    send(&mut d, &mut editor, "M-g g C-x C-f");
    let prompt = editor.prompt().unwrap();
    assert_eq!(prompt.message(), Some("Not in the minibuffer"));
}
