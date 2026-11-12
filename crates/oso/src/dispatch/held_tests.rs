//! Scenario tests of `RET` and `TAB` held while the completion worker matches (ADR 0096), in a
//! directory past 5000 names, each job run on the test's thread, and of `RET` and `M-RET` held
//! in a prompt for a directory until the listing or the look at a link that decides its answer
//! has come (ADR 0114).
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use std::path::PathBuf;
use std::sync::Arc;

use oso_core::buffer::Buffer;
use oso_core::complete::{Name, NameKind};
use oso_core::editor::Editor;
use oso_core::window::ViewSize;

use super::Dispatcher;
use super::motion_tests::send;
use super::project_tests::{in_oso, list_directories};

/// A dispatcher and an editor on `/big/a.md`, with `C-x C-f` open on `/big/` and its 6000
/// names built.
fn finding() -> (Dispatcher, Editor) {
    let mut buffer = Buffer::visiting(PathBuf::from("/big/a.md"));
    buffer.finish_empty();
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(24, 80));
    let mut d = Dispatcher::new();
    send(&mut d, &mut editor, "C-x C-f");
    let names: Arc<[Name]> = (0..6000)
        .map(|index| Name {
            name: format!("file-{index:04}.txt"),
            kind: NameKind::File,
        })
        .chain([Name {
            name: "sub".to_owned(),
            kind: NameKind::Directory,
        }])
        .collect();
    for directory in editor.take_file_requests() {
        assert_eq!(directory, PathBuf::from("/big"));
        editor.file_listed(directory, Some(Arc::clone(&names)), false);
    }
    work(&mut d, &mut editor);
    assert!(!editor.completion_pending());
    (d, editor)
}

/// Runs every job left, as the worker would, and what was held for it, as the host does.
fn work(d: &mut Dispatcher, editor: &mut Editor) {
    for _ in 0..8 {
        let Some(job) = editor.take_completion_job() else {
            return;
        };
        if let Some(report) = job.run() {
            editor.completion_reported(report);
        }
        d.run_held_completion(editor);
    }
}

#[test]
fn ret_while_a_match_is_pending_is_held_and_opens_the_newest_match() {
    let (mut d, mut editor) = finding();
    send(&mut d, &mut editor, "0 0 4 2");
    assert!(editor.completion_pending());
    send(&mut d, &mut editor, "RET");
    assert!(d.holds_for_completion());
    assert!(
        editor.prompt().is_some(),
        "nothing taken from the rows of before"
    );
    assert!(editor.take_visits().is_empty());
    work(&mut d, &mut editor);
    assert!(editor.prompt().is_none());
    assert_eq!(
        editor.take_visits(),
        [(PathBuf::from("/big/file-0042.txt"), crate::loader::UNASKED)]
    );
}

#[test]
fn tab_while_a_match_is_pending_puts_the_newest_match_in() {
    let (mut d, mut editor) = finding();
    send(&mut d, &mut editor, "s u b");
    send(&mut d, &mut editor, "TAB");
    assert!(d.holds_for_completion());
    work(&mut d, &mut editor);
    let id = editor.prompt().unwrap().buffer();
    assert_eq!(editor.buffer(id).unwrap().text().to_string(), "/big/sub/");
}

#[test]
fn c_g_drops_the_held_key_and_does_nothing_more() {
    let (mut d, mut editor) = finding();
    send(&mut d, &mut editor, "0 0 4 2 RET C-g");
    assert!(!d.holds_for_completion());
    assert!(editor.prompt().is_some(), "the prompt stays");
    work(&mut d, &mut editor);
    assert!(editor.prompt().is_some(), "and the result opens nothing");
    assert!(editor.take_visits().is_empty());
    send(&mut d, &mut editor, "C-g");
    assert!(editor.prompt().is_none(), "a second C-g quits");
}

#[test]
fn an_edit_or_a_move_drops_the_held_key() {
    for after in ["1", "C-n", "DEL"] {
        let (mut d, mut editor) = finding();
        send(&mut d, &mut editor, "0 0 4 RET");
        assert!(d.holds_for_completion());
        send(&mut d, &mut editor, after);
        assert!(!d.holds_for_completion(), "{after}");
        work(&mut d, &mut editor);
        assert!(editor.prompt().is_some(), "{after}");
    }
}

#[test]
fn m_ret_is_not_held() {
    let (mut d, mut editor) = finding();
    send(&mut d, &mut editor, "n e w M-RET");
    assert!(!d.holds_for_completion());
    assert!(editor.prompt().is_none());
    assert_eq!(editor.take_visits()[0].0, PathBuf::from("/big/new"));
}

#[test]
fn a_prefix_an_unbound_key_or_a_cancelled_one_drops_the_held_key() {
    for after in ["C-x", "<f35>", "C-x C-g"] {
        let (mut d, mut editor) = finding();
        send(&mut d, &mut editor, "0 0 4 2 RET");
        assert!(d.holds_for_completion());
        send(&mut d, &mut editor, after);
        assert!(!d.holds_for_completion(), "{after}");
        work(&mut d, &mut editor);
        assert!(editor.prompt().is_some(), "{after}");
    }
}

#[test]
fn ret_after_a_move_made_while_waiting_opens_the_row_moved_to() {
    let (mut d, mut editor) = finding();
    // The rows shown are the whole list's, `sub/` first, so this moves to `file-0001.txt`,
    // which `00` still matches.
    send(&mut d, &mut editor, "0 0 C-n C-n RET");
    work(&mut d, &mut editor);
    assert_eq!(
        editor.take_visits()[0].0,
        PathBuf::from("/big/file-0001.txt")
    );
}

#[test]
fn ret_and_m_ret_in_a_directory_prompt_wait_for_the_listing_that_decides_the_answer() {
    for key in ["RET", "M-RET"] {
        let (mut d, mut editor) = in_oso(None);
        send(&mut d, &mut editor, "C-u M-s r");
        // `~/src/oso/` is typed, and its listing has not come.
        assert!(editor.directory_answer_waits());
        send(&mut d, &mut editor, key);
        assert!(d.holds_for_completion(), "{key}");
        assert_eq!(editor.prompt().unwrap().label(), "Search in directory:");
        list_directories(&mut editor);
        d.run_held_completion(&mut editor);
        assert!(!d.holds_for_completion(), "{key}");
        assert_eq!(editor.prompt().unwrap().label(), "Search oso:", "{key}");
    }
}

#[test]
fn a_directory_prompts_answer_from_the_socket_is_refused_while_it_waits() {
    let (mut d, mut editor) = in_oso(None);
    send(&mut d, &mut editor, "C-u M-s r");
    for name in ["exit-minibuffer", "exit-minibuffer-input"] {
        let (_, result) = d.run_command(&mut editor, name, None);
        assert!(
            result.is_err_and(|error| error.contains("still on its way")),
            "{name}"
        );
        assert_eq!(editor.prompt().unwrap().label(), "Search in directory:");
    }
    // Once listed, it waits no more, whatever else may refuse it.
    list_directories(&mut editor);
    let (_, result) = d.run_command(&mut editor, "exit-minibuffer", None);
    assert!(!matches!(&result, Err(error) if error.contains("still on its way")));
}

#[test]
fn m_ret_for_a_file_is_not_held_while_its_directory_is_listed() {
    let mut buffer = Buffer::visiting(PathBuf::from("/big/a.md"));
    buffer.finish_empty();
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(24, 80));
    let mut d = Dispatcher::new();
    send(&mut d, &mut editor, "C-x C-f n e w");
    assert!(!editor.take_file_requests().is_empty(), "not listed yet");
    send(&mut d, &mut editor, "M-RET");
    assert!(!d.holds_for_completion());
    assert!(editor.prompt().is_none());
    assert_eq!(editor.take_visits()[0].0, PathBuf::from("/big/new"));
}

#[test]
fn ret_on_a_link_in_a_directory_prompt_waits_for_its_look_past_other_results() {
    use oso_core::editor::Looked;
    let (mut d, mut editor) = in_oso(None);
    send(&mut d, &mut editor, "C-u M-s r");
    for directory in editor.take_file_requests() {
        let names = [Name {
            name: "ln".to_owned(),
            kind: NameKind::Link,
        }];
        editor.file_listed(directory, Some(names.into()), false);
    }
    send(&mut d, &mut editor, "l n RET");
    assert!(d.holds_for_completion());
    // A result that is not the look ends no wait.
    d.run_held_completion(&mut editor);
    assert!(d.holds_for_completion(), "the look has not come");
    editor.take_file_looks();
    let looked = Looked {
        annotation: String::new(),
        bytes: 0,
        directory: true,
    };
    editor.file_looked(
        std::path::Path::new("/home/me/src/oso"),
        vec![("ln".to_owned(), looked)],
    );
    d.run_held_completion(&mut editor);
    assert!(!d.holds_for_completion());
    let prompt = editor.prompt().unwrap();
    assert_eq!(prompt.label(), "Search in directory:", "gone into");
    let text = editor.buffer(prompt.buffer()).unwrap().text().to_string();
    assert_eq!(text, "~/src/oso/ln/");
}
