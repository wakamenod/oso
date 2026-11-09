//! Scenario tests of completion (ADR 0076): `M-x` run by keys, its prefix argument, a command
//! it runs that reads an answer, its history, and moving through the candidates.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU.

use oso_core::buffer::Buffer;
use oso_core::complete::Candidate;
use oso_core::editor::Editor;
use oso_core::window::ViewSize;

use super::Dispatcher;
use super::motion_tests::{cursors, send};

const LINES: &str = "one\ntwo\nthree\nfour\n";

fn on(text: &str) -> (Dispatcher, Editor) {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_view_size(ViewSize::new(10, 60));
    (Dispatcher::new(), editor)
}

/// The keys that type `text`, a space as `SPC`.
fn typing(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            ' ' => "SPC".to_owned(),
            c => c.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn selected(editor: &Editor) -> Option<&str> {
    let candidates = editor.prompt()?.candidates()?;
    candidates.selected_candidate().map(Candidate::text)
}

#[test]
fn m_x_runs_the_command_typed() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-x");
    let prompt = editor.prompt().unwrap();
    assert_eq!(prompt.label(), "M-x");
    let total = prompt.candidates().unwrap().total();
    assert!(total > 30, "{total} commands");
    send(&mut d, &mut editor, &typing("forward-char"));
    assert_eq!(selected(&editor), Some("forward-char"));
    send(&mut d, &mut editor, "RET");
    assert!(editor.prompt().is_none());
    assert_eq!(cursors(&editor), [1]);
}

#[test]
fn components_match_in_any_order_and_c_n_moves() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-x");
    send(&mut d, &mut editor, &typing("line next"));
    assert_eq!(selected(&editor), Some("next-line"));
    send(&mut d, &mut editor, "C-g");
    send(&mut d, &mut editor, "M-x");
    send(&mut d, &mut editor, &typing("end-of-"));
    let first = selected(&editor).unwrap().to_owned();
    send(&mut d, &mut editor, "C-n");
    let second = selected(&editor).unwrap().to_owned();
    assert_ne!(first, second);
    send(&mut d, &mut editor, "<up>");
    assert_eq!(selected(&editor), Some(first.as_str()));
    send(&mut d, &mut editor, "C-g");
    assert!(editor.prompt().is_none());
}

#[test]
fn the_prefix_argument_goes_to_the_command_m_x_runs() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "C-u 3 M-x");
    assert_eq!(editor.prompt().unwrap().label(), "3 M-x");
    send(&mut d, &mut editor, &typing("forward-char"));
    send(&mut d, &mut editor, "RET");
    assert_eq!(cursors(&editor), [3]);
    // A number stands for what the command would read.
    send(&mut d, &mut editor, "M-3 M-x");
    send(&mut d, &mut editor, &typing("goto-line"));
    send(&mut d, &mut editor, "RET");
    assert!(editor.prompt().is_none());
    assert_eq!(cursors(&editor), ["one\ntwo\n".len()]);
}

#[test]
fn a_command_that_reads_opens_its_prompt_after_m_x_closes() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-x");
    send(&mut d, &mut editor, &typing("goto-line"));
    send(&mut d, &mut editor, "RET");
    assert_eq!(editor.prompts().len(), 1);
    assert_eq!(editor.prompt().unwrap().label(), "Go to line: ");
    send(&mut d, &mut editor, "2 RET");
    assert_eq!(cursors(&editor), ["one\n".len()]);
}

#[test]
fn the_command_run_last_comes_first_and_ret_runs_it_again() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-x");
    send(&mut d, &mut editor, &typing("forward-char"));
    send(&mut d, &mut editor, "RET M-x");
    assert_eq!(selected(&editor), Some("forward-char"));
    send(&mut d, &mut editor, "RET");
    assert_eq!(cursors(&editor), [2]);
    // M-p puts it in the answer, which then matches it exactly.
    send(&mut d, &mut editor, "M-x M-p");
    assert_eq!(selected(&editor), Some("forward-char"));
    send(&mut d, &mut editor, "C-g");
}

#[test]
fn tab_puts_the_selected_name_in_the_answer() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-x");
    send(&mut d, &mut editor, &typing("goto-l"));
    send(&mut d, &mut editor, "TAB");
    let prompt = editor.prompt().unwrap();
    let answer = editor.buffer(prompt.buffer()).unwrap().text().to_string();
    assert_eq!(answer, "goto-line");
}

#[test]
fn with_nothing_matching_ret_says_so_and_stays() {
    let (mut d, mut editor) = on(LINES);
    send(&mut d, &mut editor, "M-x");
    send(&mut d, &mut editor, &typing("qqqq"));
    send(&mut d, &mut editor, "RET");
    let prompt = editor.prompt().unwrap();
    assert_eq!(prompt.message(), Some("No match"));
    assert_eq!(prompt.candidates().unwrap().selected(), None);
}

#[cfg(feature = "dev")]
mod socket {
    use super::*;

    #[test]
    fn get_state_shows_the_count_the_selection_and_the_rows_shown() {
        let (mut d, mut editor) = on(LINES);
        send(&mut d, &mut editor, "M-x");
        send(&mut d, &mut editor, &typing("char"));
        send(&mut d, &mut editor, "C-n");
        let state = crate::dev::get_state(&editor, &[], None);
        let prompt = &state["prompts"][0];
        assert_eq!(prompt["kind"], "complete");
        let completion = &prompt["completion"];
        let count = completion["count"].as_u64().unwrap();
        assert!(count >= 2 && count < completion["total"].as_u64().unwrap());
        assert_eq!(completion["selected"], 1);
        let rows = completion["rows"].as_array().unwrap();
        assert_eq!(rows.len() as u64, count.min(18));
        assert!(
            rows.iter()
                .all(|row| row.as_str().unwrap().contains("char"))
        );
    }

    #[test]
    fn beside_a_secret_get_state_leaves_the_candidates_out() {
        let (mut d, mut editor) = on(LINES);
        // A passphrase prompt beneath is a buffer that may hold secrets.
        send(&mut d, &mut editor, "<f34> a M-x");
        send(&mut d, &mut editor, &typing("char"));
        assert_eq!(editor.prompts().len(), 2);
        let state = crate::dev::get_state(&editor, &[], None);
        assert_eq!(state["prompts"][1]["kind"], "complete");
        assert!(state["prompts"][1]["completion"].is_null());
        assert!(state["prompts"][1]["text"].is_null());
    }

    #[test]
    fn beside_a_secret_run_command_runs_only_c_g_in_a_prompt_that_completes() {
        let path = std::path::PathBuf::from("/tmp/.env");
        let mut buffer = Buffer::visiting(path.clone());
        buffer.apply_load(oso_core::load::decode_all(b"TOKEN=x\n", Default::default()));
        buffer.judge_secret(Some(&[path]));
        let mut editor = Editor::with_buffer(buffer);
        editor.set_view_size(ViewSize::new(10, 60));
        let mut d = Dispatcher::new();
        send(&mut d, &mut editor, "M-x");
        send(&mut d, &mut editor, &typing("char"));
        for name in ["next-completion", "exit-minibuffer", "insert-completion"] {
            let (_, result) = d.run_command(&mut editor, name, None);
            assert!(result.unwrap_err().contains("only keyboard-quit"), "{name}");
        }
        let (_, result) = d.run_command(&mut editor, "keyboard-quit", None);
        assert_eq!(result, Ok(()));
        assert!(editor.prompt().is_none());
    }

    #[test]
    fn run_command_refuses_m_x_and_gives_a_count_to_a_command_that_reads_its_key() {
        let (mut d, mut editor) = on(LINES);
        let (_, result) = d.run_command(&mut editor, "execute-extended-command", None);
        assert!(result.unwrap_err().contains("run the command itself"));
        assert!(editor.prompt().is_none());
        // The count gets through; with no key there is nothing to insert.
        let (_, result) = d.run_command(&mut editor, "self-insert-command", Some(2));
        assert_eq!(
            result.unwrap_err(),
            "self-insert-command: No text to insert"
        );
    }
}

/// The app's own features match Perl classes and general categories: tests elsewhere in the
/// workspace pull in more of `regex-syntax`'s tables than the app has (ADR 0076).
#[test]
fn perl_classes_and_general_categories_match_with_the_apps_features() {
    let items: std::sync::Arc<[Candidate]> = ["forward-char", "goto-line", "日本語", "line42"]
        .map(|text| Candidate::new(text, "", ""))
        .into();
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", LINES));
    for (input, first) in [
        (r"\w+-ch", "forward-char"),
        (r"\d+", "line42"),
        (r"\bline\b", "goto-line"),
        (r"\pL{3}$", "日本語"),
    ] {
        editor
            .open_completion("M-x", "command", items.clone())
            .unwrap();
        let id = editor.prompt().unwrap().buffer();
        editor
            .apply_edits(
                id,
                &[oso_core::buffer::Replacement {
                    range: 0..0,
                    text: input,
                }],
            )
            .unwrap();
        editor.match_completion();
        assert_eq!(selected(&editor), Some(first), "{input}");
        editor.quit();
    }
}
